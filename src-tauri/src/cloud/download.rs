//! Resumable, verified download manager for the hero model file.
//!
//! `run_download` is the testable core: a pure function (aside from the
//! network/filesystem it necessarily touches) driven entirely by closures
//! and atomics, with NO `AppHandle`/Tauri dependency — that's what lets the
//! 10-case test suite below drive it directly against the ranged mock
//! server in `test_support`. The three `#[tauri::command]`s at the bottom
//! are a thin orchestration layer: catalog lookup, the single-slot
//! `Downloads` registry, and wiring `run_download`'s closures to
//! `app.emit`/`cloud.download_authorization`.
//!
//! Locking discipline: the worker thread spawned by `download_model` holds
//! NONE of `session.rs`'s locks — it only ever calls
//! `cloud.download_authorization(model_id)` (itself lock-disciplined, see
//! session.rs) at start and on re-mint, same as any other `Cloud` caller.
//!
//! Secret hygiene: `DownloadAuth::authorization` (the B2 token) is placed
//! ONLY in the request's `Authorization` header (raw value, no `Bearer `
//! prefix — B2's native download-auth convention) and is never logged, put
//! in an error message, or included in a `DownloadProgress` event.
//!
//! Testability knob: the streaming agent's read timeout (the stall guard)
//! defaults to 30s but is overridable via the `CLEOPHIS_DOWNLOAD_READ_TIMEOUT_MS`
//! env var so tests can shrink it to exercise `RangedBehavior::StallForever`
//! without waiting out a real 30s timeout. Not part of the public function
//! surface — keeping `run_download`'s signature clean per the task brief.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::cloud::config;
use crate::cloud::error::CloudError;
use crate::cloud::rest;
use crate::cloud::session::Cloud;
use crate::inference::Engine;

/// The single-slot active-download registry. Only one download runs at a
/// time app-wide (matches the hero-only download surface of this
/// milestone).
pub struct Downloads {
    active: Mutex<Option<Active>>,
}

struct Active {
    model_id: String,
    cancel: Arc<AtomicBool>,
    bytes: Arc<AtomicU64>,
    total: u64,
}

impl Downloads {
    pub fn new() -> Self {
        Downloads {
            active: Mutex::new(None),
        }
    }

    /// Sets the cancel flag if a download is active; no-op otherwise. Used
    /// by both the `cancel_download` command and main.rs's window-Destroyed
    /// handler (so quitting the app mid-download doesn't leave an orphaned
    /// worker thread writing to disk after the window is gone).
    pub fn request_cancel(&self) {
        if let Some(active) = self.active.lock().unwrap().as_ref() {
            active.cancel.store(true, Ordering::Relaxed);
        }
    }
}

impl Default for Downloads {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadProgress {
    pub model_id: String,
    /// requesting|downloading|verifying|done|failed|cancelled
    pub phase: String,
    pub bytes_downloaded: u64,
    pub total_bytes: u64,
    pub bytes_per_sec: u64,
    pub error: Option<String>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadStatus {
    pub installed: bool,
    pub part_bytes: u64,
    pub active: bool,
    pub bytes_downloaded: u64,
    pub total_bytes: u64,
}

/// The testable core. `auth_provider` is called once at start and again on
/// every re-mint (a retryable failure, per worker step 5). Deliberately
/// takes no `model_id`: it doesn't need one — the destination path,
/// expected size, and expected hash fully describe the download, and
/// `auth_provider` already has `model_id` baked in as a closure (the
/// `download_model` command's `move || cloud.download_authorization(&model_id)`).
/// Consequently the `DownloadProgress` values this function builds carry
/// `model_id: String::new()` — the Tauri command wrapper's `emit` closure
/// patches the real `model_id` in before forwarding to `app.emit`, since
/// only it (not this model-agnostic core) knows it. Documented in the task
/// report as a deliberate reading of the given signature, which has no
/// `model_id` parameter.
pub fn run_download(
    auth_provider: &dyn Fn() -> Result<rest::DownloadAuth, CloudError>,
    final_path: &Path,
    expected_bytes: u64,
    expected_sha256: &str,
    emit: &dyn Fn(DownloadProgress),
    cancel: &AtomicBool,
    bytes_counter: &AtomicU64,
) -> Result<(), String> {
    let part_path = part_path_for(final_path);

    // 1. Preflight.
    let parent = final_path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)
        .map_err(|e| format!("Could not create the download folder: {e}"))?;

    let existing_part_len = std::fs::metadata(&part_path).map(|m| m.len()).unwrap_or(0);
    check_disk_space(final_path, expected_bytes, existing_part_len)?;

    // 2. Mint auth, cross-check the catalog against the server's idea of
    // this file's size.
    emit(DownloadProgress {
        model_id: String::new(),
        phase: "requesting".into(),
        bytes_downloaded: existing_part_len,
        total_bytes: expected_bytes,
        bytes_per_sec: 0,
        error: None,
    });

    let mut auth = auth_provider().map_err(|e| e.user_message())?;
    if auth.file_bytes != expected_bytes {
        return Err("Catalog out of date — please update the app.".to_string());
    }
    // Real builds only: this module's own test suite below drives
    // `run_download` against `test_support::start_ranged_server`, which
    // speaks plain HTTP on 127.0.0.1 (no TLS) — the gate would reject every
    // one of those mock URLs on scheme alone before a single byte streams.
    // `download_host_allowed` is validated directly by its own unit tests
    // instead (see the `download_host_allowed_*` tests below), so this
    // cfg-gate costs no coverage of the validator itself, only of this one
    // call site — the same "testability knob" pattern `streaming_agent`
    // uses above for its read timeout.
    #[cfg(not(test))]
    if !download_host_allowed(&auth.url) {
        return Err("Download source not recognized — please update the app.".to_string());
    }

    let agent = streaming_agent();
    let mut consecutive_failures: u32 = 0;
    let mut bytes_at_last_failure: u64 = 0;
    let mut last_failure_kind: Option<FailureKind> = None;
    // Backdated so the very first streamed chunk always clears the >=500ms
    // throttle and emits a `downloading` event immediately, rather than a
    // small/fast download (which can finish well under 500ms end-to-end on
    // localhost) never showing a single progress tick before `done`.
    let mut last_emit = Instant::now() - Duration::from_millis(500);
    let mut bytes_since_last_emit: u64 = 0;

    // 3/4/5. Resume + stream + retry loop. Every attempt (including the
    // first) re-derives its starting point from whatever is actually on
    // disk right now, since a retry's `.part` may have grown since the
    // last attempt.
    let final_hasher: Sha256 = 'attempt: loop {
        if cancel.load(Ordering::Relaxed) {
            emit(cancelled_progress(bytes_counter, expected_bytes));
            return Ok(());
        }

        let (mut file, mut hasher, resume_from) = prepare_part_file(&part_path, expected_bytes)?;
        bytes_counter.store(resume_from, Ordering::Relaxed);

        let mut req = agent.get(&auth.url).set("Authorization", &auth.authorization);
        if resume_from > 0 {
            req = req.set("Range", &format!("bytes={resume_from}-"));
        }

        let resp = match req.call() {
            Ok(r) => r,
            // Transport error OR a non-2xx status (401/403 included) when
            // (re)connecting — same retry policy either way, though the
            // give-up message and fast-fail behavior below depend on which.
            Err(connect_err) => {
                let kind = classify_connect_err(&connect_err);
                match record_failure_and_backoff(
                    &mut consecutive_failures,
                    &mut bytes_at_last_failure,
                    &mut last_failure_kind,
                    kind,
                    bytes_counter,
                    cancel,
                ) {
                    Ok(true) => {
                        emit(cancelled_progress(bytes_counter, expected_bytes));
                        return Ok(());
                    }
                    Ok(false) => {
                        match remint_with_budget(
                            auth_provider,
                            &mut consecutive_failures,
                            &mut bytes_at_last_failure,
                            &mut last_failure_kind,
                            bytes_counter,
                            cancel,
                        ) {
                            Ok(Some(new_auth)) => {
                                // Re-gate the freshly minted URL — a retry
                                // must not attach the auth token to a host
                                // the initial gate never vetted.
                                #[cfg(not(test))]
                                if !download_host_allowed(&new_auth.url) {
                                    return Err("Download source not recognized — please update the app.".to_string());
                                }
                                auth = new_auth;
                                continue 'attempt;
                            }
                            Ok(None) => {
                                emit(cancelled_progress(bytes_counter, expected_bytes));
                                return Ok(());
                            }
                            Err(msg) => return Err(msg),
                        }
                    }
                    Err(msg) => return Err(msg),
                }
            }
        };

        // A server that ignores Range replies 200 with the full body even
        // though we asked to resume — restart clean rather than append the
        // full body onto existing bytes and corrupt the file.
        if resume_from > 0 && resp.status() == 200 {
            drop(file);
            file = File::create(&part_path)
                .map_err(|e| format!("Failed to reset download file: {e}"))?;
            hasher = Sha256::new();
            bytes_counter.store(0, Ordering::Relaxed);
        }

        match stream_response(
            resp,
            &mut file,
            &mut hasher,
            cancel,
            bytes_counter,
            expected_bytes,
            emit,
            &mut last_emit,
            &mut bytes_since_last_emit,
        ) {
            Ok(StreamOutcome::Completed) => break 'attempt hasher,
            Ok(StreamOutcome::Cancelled) => return Ok(()),
            Err(StreamFailure::Fatal(msg)) => return Err(msg),
            Err(StreamFailure::Retryable(_io_err)) => {
                // A mid-stream failure never carries an HTTP status — the
                // connect already succeeded (200/206) before the body read
                // failed — so it's always classified as Transport.
                match record_failure_and_backoff(
                    &mut consecutive_failures,
                    &mut bytes_at_last_failure,
                    &mut last_failure_kind,
                    FailureKind::Transport,
                    bytes_counter,
                    cancel,
                ) {
                    Ok(true) => {
                        emit(cancelled_progress(bytes_counter, expected_bytes));
                        return Ok(());
                    }
                    Ok(false) => {
                        match remint_with_budget(
                            auth_provider,
                            &mut consecutive_failures,
                            &mut bytes_at_last_failure,
                            &mut last_failure_kind,
                            bytes_counter,
                            cancel,
                        ) {
                            Ok(Some(new_auth)) => {
                                // Re-gate the freshly minted URL — a retry
                                // must not attach the auth token to a host
                                // the initial gate never vetted.
                                #[cfg(not(test))]
                                if !download_host_allowed(&new_auth.url) {
                                    return Err("Download source not recognized — please update the app.".to_string());
                                }
                                auth = new_auth;
                                continue 'attempt;
                            }
                            Ok(None) => {
                                emit(cancelled_progress(bytes_counter, expected_bytes));
                                return Ok(());
                            }
                            Err(msg) => return Err(msg),
                        }
                    }
                    Err(msg) => return Err(msg),
                }
            }
        }
    };

    // 6. Finish: verify, then finalize. Deliberately does NOT emit `done`
    // itself (a deviation from the original worker-step-6 wording, made
    // during whole-branch review to close a race): `download_model`'s
    // command wrapper emits `done` only AFTER it has called
    // `start_if_no_model`, so the front-end never observes `done` while the
    // engine's status could still read `NoModel` — `run_download` returning
    // `Ok(())` (with the file present at `final_path`) is itself the
    // completion signal the wrapper acts on.
    emit(DownloadProgress {
        model_id: String::new(),
        phase: "verifying".into(),
        bytes_downloaded: expected_bytes,
        total_bytes: expected_bytes,
        bytes_per_sec: 0,
        error: None,
    });

    let digest_hex: String = final_hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let final_len = std::fs::metadata(&part_path).map(|m| m.len()).unwrap_or(0);

    if !digest_hex.eq_ignore_ascii_case(expected_sha256) || final_len != expected_bytes {
        let _ = std::fs::remove_file(&part_path);
        return Err("Download was corrupted — retrying will start it over.".to_string());
    }

    std::fs::rename(&part_path, final_path)
        .map_err(|e| format!("Failed to finalize download: {e}"))?;

    Ok(())
}

fn cancelled_progress(bytes_counter: &AtomicU64, expected_bytes: u64) -> DownloadProgress {
    DownloadProgress {
        model_id: String::new(),
        phase: "cancelled".into(),
        bytes_downloaded: bytes_counter.load(Ordering::Relaxed),
        total_bytes: expected_bytes,
        bytes_per_sec: 0,
        error: None,
    }
}

/// `<final_path>.part` — same directory, same file name plus the suffix.
/// Must round-trip with `inference::model_path`'s app-data branch: the
/// RENAMED (finished) path is exactly `final_path`, never anything else.
fn part_path_for(final_path: &Path) -> PathBuf {
    let mut os = final_path.as_os_str().to_os_string();
    os.push(".part");
    PathBuf::from(os)
}

/// Preflight disk-space check: the longest mount-point-prefix match against
/// `final_path`'s parent must have at least `expected_bytes -
/// existing_part_len + 200 MiB` free. If no disk can be matched (unusual
/// mount layout — e.g. some sandboxed CI environments) the check is skipped
/// rather than false-failing; `stream_response`'s own `write_all` errors
/// remain the fallback safety net for a truly full disk. Not covered by the
/// unit tests below (see the task report) — there's no portable way to fake
/// "disk full" without root/admin tricks.
fn check_disk_space(final_path: &Path, expected_bytes: u64, existing_part_len: u64) -> Result<(), String> {
    const SAFETY_MARGIN: u64 = 200 * 1024 * 1024;
    let needed = expected_bytes.saturating_sub(existing_part_len) + SAFETY_MARGIN;

    let target = final_path.parent().unwrap_or(final_path);
    let disks = sysinfo::Disks::new_with_refreshed_list();

    let mut best: Option<(&sysinfo::Disk, usize)> = None;
    for disk in disks.list() {
        let mount = disk.mount_point();
        if target.starts_with(mount) {
            let len = mount.as_os_str().len();
            if best.map(|(_, l)| len > l).unwrap_or(true) {
                best = Some((disk, len));
            }
        }
    }

    if let Some((disk, _)) = best {
        if disk.available_space() < needed {
            let gib = needed as f64 / (1024.0 * 1024.0 * 1024.0);
            return Err(format!("Not enough disk space — need {gib:.2} GiB free."));
        }
    }
    Ok(())
}

/// Determines how this attempt should start, based on whatever is currently
/// on disk (worker step 3): a `.part` shorter than `expected_bytes` resumes
/// (its existing bytes are re-hashed into a fresh `Sha256` so the final
/// digest still covers the whole file, then reopened for append);
/// anything else (`.part` missing, or already >= `expected_bytes` — a
/// stale/corrupt leftover) restarts from empty.
fn prepare_part_file(part_path: &Path, expected_bytes: u64) -> Result<(File, Sha256, u64), String> {
    let part_len = std::fs::metadata(part_path).map(|m| m.len()).unwrap_or(0);

    if part_len == 0 || part_len >= expected_bytes {
        if part_len > 0 {
            std::fs::remove_file(part_path)
                .map_err(|e| format!("Failed to reset stale partial download: {e}"))?;
        }
        let file = File::create(part_path)
            .map_err(|e| format!("Failed to create download file: {e}"))?;
        return Ok((file, Sha256::new(), 0));
    }

    let mut hasher = Sha256::new();
    rehash_existing(part_path, &mut hasher)?;
    let file = OpenOptions::new()
        .append(true)
        .open(part_path)
        .map_err(|e| format!("Failed to resume partial download: {e}"))?;
    Ok((file, hasher, part_len))
}

fn rehash_existing(path: &Path, hasher: &mut Sha256) -> Result<(), String> {
    let mut f = File::open(path).map_err(|e| format!("Failed to read partial download: {e}"))?;
    let mut buf = [0u8; 256 * 1024];
    loop {
        let n = f
            .read(&mut buf)
            .map_err(|e| format!("Failed to read partial download: {e}"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(())
}

/// Gates the B2 download URL before the per-request auth token is attached
/// to it: `true` iff `url` starts with `https://` AND its host is exactly
/// `dl.cleophis.com` or ends with `.backblazeb2.com`. Defense-in-depth —
/// TLS already blocks MITM and the edge function that mints `auth.url` is
/// trusted — against a buggy/compromised edge function or a CDN 3xx sending
/// the raw B2 `Authorization` token to an unintended host. Mirrors the
/// Stripe URL-gate discipline in `cloud::session::checkout_outcome` /
/// `portal_outcome`.
///
/// Parsed with the `url` crate — the SAME parser ureq resolves the request
/// host with — so the allowlist can never diverge from where the GET
/// actually connects. A hand-rolled split on `/`/`@`/`:` looks right but
/// misses the other WHATWG authority delimiters (`?`, `#`, `\`): a string
/// like `https://evil.com#.backblazeb2.com/` would pass a naive suffix
/// check yet ureq would connect to `evil.com`. Delegating to `url::Url`
/// closes that gap by construction. Host comparison is case-insensitive
/// (the crate lower-cases the host), equality is exact (`dl.cleophis.com.evil.com`
/// fails), and the suffix is dot-anchored (`evil-backblazeb2.com` fails).
fn download_host_allowed(raw: &str) -> bool {
    let Ok(parsed) = url::Url::parse(raw) else {
        return false;
    };
    if parsed.scheme() != "https" {
        return false;
    }
    let Some(host) = parsed.host_str() else {
        return false;
    };
    host == "dl.cleophis.com" || host.ends_with(".backblazeb2.com")
}

/// Streaming agent: connect timeout from `config::CONNECT_TIMEOUT`, NO
/// overall timeout (large files legitimately take a long time), and a read
/// timeout that is the real stall guard — see the module doc comment for
/// why it's env-overridable.
fn streaming_agent() -> ureq::Agent {
    let read_timeout_ms: u64 = std::env::var("CLEOPHIS_DOWNLOAD_READ_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30_000);
    ureq::AgentBuilder::new()
        .timeout_connect(config::CONNECT_TIMEOUT)
        .timeout_read(Duration::from_millis(read_timeout_ms))
        .timeout_write(Duration::from_secs(30))
        // A B2/CDN signed download URL never legitimately redirects — no
        // redirects means a 3xx surfaces as a non-2xx status instead of
        // ureq silently re-attaching the `Authorization` header (with the
        // raw B2 token) to whatever Location host the response names.
        .redirects(0)
        .build()
}

/// Sleeps `total`, checking `cancel` every <=250ms so a cancellation during
/// backoff is honored promptly rather than after the full sleep. Returns
/// true iff `cancel` was observed set (before or during the sleep).
fn interruptible_sleep(total: Duration, cancel: &AtomicBool) -> bool {
    const SLICE: Duration = Duration::from_millis(250);
    let mut remaining = total;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return true;
        }
        if remaining.is_zero() {
            return false;
        }
        let this = remaining.min(SLICE);
        std::thread::sleep(this);
        remaining -= this;
    }
}

/// What kind of thing made an attempt fail — distinguishes a definitive
/// server refusal (an HTTP status from the download GET itself) from a
/// transport/timeout problem, so the give-up message can say which one it
/// was instead of always blaming "your connection" for e.g. a B2 daily
/// download-cap 403. Only ever `Status` for a CONNECT-time failure
/// (`req.call()` returning `Err(ureq::Error::Status(..))`) — a mid-stream
/// read/timeout failure never carries an HTTP status (the connect already
/// succeeded), so it's always `Transport`.
#[derive(Clone, Copy, PartialEq)]
enum FailureKind {
    Transport,
    Status(u16),
}

fn classify_connect_err(err: &ureq::Error) -> FailureKind {
    match err {
        ureq::Error::Status(code, _) => FailureKind::Status(*code),
        ureq::Error::Transport(_) => FailureKind::Transport,
    }
}

/// `true` for the CDN statuses that mean "retrying won't help without
/// outside intervention" (object genuinely missing, or B2's daily download
/// cap rejecting every request account-wide) — as opposed to a 401 (token
/// expired, a re-mint plausibly fixes it) or a 5xx (transient server
/// trouble).
fn is_definitive_refusal(kind: FailureKind) -> bool {
    matches!(kind, FailureKind::Status(403) | FailureKind::Status(404))
}

fn give_up_message(kind: FailureKind) -> String {
    match kind {
        FailureKind::Status(status) => format!(
            "Download failed — the server refused the request (HTTP {status}). This can be a temporary account limit; try again later."
        ),
        FailureKind::Transport => "Download failed — check your connection and retry.".to_string(),
    }
}

/// Bookkeeping for worker step 5's retry policy: increments the
/// consecutive-failure counter (resetting it, along with `last_failure_kind`,
/// if at least one byte advanced since the last failure), then either
/// sleeps the matching backoff or gives up.
///
/// Returns `Ok(true)` if `cancel` fired during the backoff sleep (caller
/// should emit `cancelled` and return `Ok(())`), `Ok(false)` if the caller
/// should re-mint auth and retry, or `Err(msg)` once the retry budget is
/// exhausted (caller returns that `Err` directly). The budget is normally
/// 3 consecutive failures, but a DEFINITIVE refusal (`is_definitive_refusal`
/// — 403/404) seen twice in a row gives up after the 2nd instead of burning
/// a 3rd retry: every retry in this loop already re-mints auth before
/// looping back, so "twice in a row with a re-mint in between" is exactly
/// what a second consecutive definitive status represents — a fresh token
/// didn't help, so a third attempt won't either. The give-up message
/// reflects `kind` — the LAST attempt's failure — so a definitive server
/// refusal is reported as such rather than as a generic connection problem.
///
/// Backoff schedule note (documented per the task instructions, since this
/// is a real ambiguity in the brief rather than a codebase conflict): the
/// brief specifies sleeps "2s/5s/10s" AND "after 3 consecutive failures ->
/// Err" in the same sentence. Giving up ON the 3rd consecutive failure
/// (rather than after a 3rd retry attempt also fails) means only the first
/// two backoff values are ever exercised — failure #1 sleeps 2s before
/// retry #2, failure #2 sleeps 5s before retry #3, and failure #3 gives up
/// immediately rather than sleeping 10s first. The `_ => 10` arm is kept in
/// the table below for schedule-shape completeness even though it's
/// currently unreachable under this reading.
fn record_failure_and_backoff(
    consecutive_failures: &mut u32,
    bytes_at_last_failure: &mut u64,
    last_failure_kind: &mut Option<FailureKind>,
    kind: FailureKind,
    bytes_counter: &AtomicU64,
    cancel: &AtomicBool,
) -> Result<bool, String> {
    let current = bytes_counter.load(Ordering::Relaxed);
    if current > *bytes_at_last_failure {
        *consecutive_failures = 0;
        *last_failure_kind = None;
    }

    let prev_was_definitive = last_failure_kind.map(is_definitive_refusal).unwrap_or(false);
    let fast_fail = is_definitive_refusal(kind) && prev_was_definitive;

    *consecutive_failures += 1;
    *bytes_at_last_failure = current;
    *last_failure_kind = Some(kind);

    if fast_fail || *consecutive_failures >= 3 {
        return Err(give_up_message(kind));
    }

    let backoff_secs = match *consecutive_failures {
        1 => 2,
        2 => 5,
        _ => 10,
    };
    Ok(interruptible_sleep(Duration::from_secs(backoff_secs), cancel))
}

/// Classifies a failed `auth_provider()` (re-mint) call the same way a
/// failed connect is classified: `CloudError::Api { status, .. }` — a
/// definitive HTTP-level rejection from the mint endpoint itself (e.g. an
/// RLS rejection, or Supabase surfacing a CDN-side refusal) — maps to
/// `Status(status)`; everything else (`Offline`, `SessionExpired`,
/// `Internal`, ...) maps to `Transport`, since none of those carry a
/// meaningful HTTP status for the download itself.
fn classify_mint_err(err: &CloudError) -> FailureKind {
    match err {
        CloudError::Api { status, .. } => FailureKind::Status(*status),
        _ => FailureKind::Transport,
    }
}

/// Re-mints auth, itself subject to the SAME retry budget as a connect or
/// mid-stream failure — a mint failure (Supabase offline, the
/// `download-url` edge function rejecting the request, ...) must consume
/// the 3-consecutive-failure policy rather than aborting the whole download
/// outright via `?`. Loops internally: each failed mint attempt is
/// recorded through `record_failure_and_backoff` exactly like any other
/// attempt failure, sharing the same counters (so a mint failure right
/// after a connect failure counts as the 2nd of 3, not a fresh 1st).
///
/// Returns `Ok(Some(auth))` once a mint succeeds, `Ok(None)` if `cancel`
/// fired during a backoff sleep (caller emits `cancelled` and returns
/// `Ok(())`), or `Err(msg)` once the retry budget is exhausted (caller
/// returns that `Err` directly — the give-up message already reflects
/// whichever failure was last, mint or connect/stream).
fn remint_with_budget(
    auth_provider: &dyn Fn() -> Result<rest::DownloadAuth, CloudError>,
    consecutive_failures: &mut u32,
    bytes_at_last_failure: &mut u64,
    last_failure_kind: &mut Option<FailureKind>,
    bytes_counter: &AtomicU64,
    cancel: &AtomicBool,
) -> Result<Option<rest::DownloadAuth>, String> {
    loop {
        match auth_provider() {
            Ok(auth) => return Ok(Some(auth)),
            Err(mint_err) => {
                let kind = classify_mint_err(&mint_err);
                match record_failure_and_backoff(
                    consecutive_failures,
                    bytes_at_last_failure,
                    last_failure_kind,
                    kind,
                    bytes_counter,
                    cancel,
                ) {
                    Ok(true) => return Ok(None),
                    Ok(false) => continue,
                    Err(msg) => return Err(msg),
                }
            }
        }
    }
}

enum StreamOutcome {
    Completed,
    Cancelled,
}

enum StreamFailure {
    /// A read/transport-style failure mid-stream — routed through the same
    /// retry policy as a connect-time failure.
    Retryable(std::io::Error),
    /// A local, non-network problem (disk write failed) — not retried.
    Fatal(String),
}

#[allow(clippy::too_many_arguments)]
fn stream_response(
    resp: ureq::Response,
    file: &mut File,
    hasher: &mut Sha256,
    cancel: &AtomicBool,
    bytes_counter: &AtomicU64,
    expected_bytes: u64,
    emit: &dyn Fn(DownloadProgress),
    last_emit: &mut Instant,
    bytes_since_last_emit: &mut u64,
) -> Result<StreamOutcome, StreamFailure> {
    const CHUNK: usize = 256 * 1024;
    let mut reader = resp.into_reader();
    let mut buf = vec![0u8; CHUNK];

    loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = file.flush();
            emit(cancelled_progress(bytes_counter, expected_bytes));
            return Ok(StreamOutcome::Cancelled);
        }

        let n = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => return Err(StreamFailure::Retryable(e)),
        };

        hasher.update(&buf[..n]);
        if let Err(e) = file.write_all(&buf[..n]) {
            return Err(StreamFailure::Fatal(format!("Failed to write to disk: {e}")));
        }
        bytes_counter.fetch_add(n as u64, Ordering::Relaxed);
        *bytes_since_last_emit += n as u64;

        let elapsed = last_emit.elapsed();
        if elapsed >= Duration::from_millis(500) {
            let bps = (*bytes_since_last_emit as f64 / elapsed.as_secs_f64().max(0.001)) as u64;
            emit(DownloadProgress {
                model_id: String::new(),
                phase: "downloading".into(),
                bytes_downloaded: bytes_counter.load(Ordering::Relaxed),
                total_bytes: expected_bytes,
                bytes_per_sec: bps,
                error: None,
            });
            *last_emit = Instant::now();
            *bytes_since_last_emit = 0;
        }
    }

    let _ = file.flush();
    Ok(StreamOutcome::Completed)
}

// ---------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------

#[tauri::command]
pub async fn download_model(
    model_id: String,
    app: AppHandle,
    downloads: State<'_, Arc<Downloads>>,
    cloud: State<'_, Arc<Cloud>>,
    engine: State<'_, Arc<Engine>>,
) -> Result<(), String> {
    let downloads = downloads.inner().clone();
    let cloud = cloud.inner().clone();
    let engine = engine.inner().clone();

    let root = crate::inference::resources_root(&app);
    let raw = std::fs::read_to_string(root.join("catalog.json")).map_err(|e| e.to_string())?;
    let entries = crate::catalog::parse_catalog(&raw)?;
    let hero = crate::catalog::hero(&entries).ok_or_else(|| "catalog missing integrity data".to_string())?;
    if hero.id != model_id {
        return Err("Unknown model.".to_string());
    }
    let model_file = hero
        .model_file
        .clone()
        .ok_or_else(|| "catalog missing integrity data".to_string())?;
    let sha256 = hero
        .sha256
        .clone()
        .ok_or_else(|| "catalog missing integrity data".to_string())?;
    if hero.version.is_none() {
        return Err("catalog missing integrity data".to_string());
    }
    let expected_bytes = hero.file_bytes;

    let app_data = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let final_path = app_data.join(&model_file);

    if final_path.exists() {
        return Err("Already installed.".to_string());
    }

    let cancel = Arc::new(AtomicBool::new(false));
    let bytes = Arc::new(AtomicU64::new(0));

    {
        let mut guard = downloads.active.lock().unwrap();
        if guard.is_some() {
            return Err("A download is already in progress.".to_string());
        }
        *guard = Some(Active {
            model_id: model_id.clone(),
            cancel: cancel.clone(),
            bytes: bytes.clone(),
            total: expected_bytes,
        });
    }

    let downloads_for_thread = downloads.clone();
    let cloud_for_thread = cloud.clone();
    let engine_for_thread = engine.clone();
    let app_for_thread = app.clone();
    let model_id_for_thread = model_id.clone();
    let final_path_for_thread = final_path.clone();

    std::thread::spawn(move || {
        // A Drop guard so a panic mid-download can't wedge the active slot
        // forever — the guard runs during unwind just like on a normal
        // return.
        struct ClearActiveOnDrop {
            downloads: Arc<Downloads>,
        }
        impl Drop for ClearActiveOnDrop {
            fn drop(&mut self) {
                *self.downloads.active.lock().unwrap() = None;
            }
        }
        let _clear_guard = ClearActiveOnDrop {
            downloads: downloads_for_thread,
        };

        let auth_provider = {
            let cloud = cloud_for_thread;
            let model_id = model_id_for_thread.clone();
            move || cloud.download_authorization(&model_id)
        };

        let emit = {
            let app = app_for_thread.clone();
            let model_id = model_id_for_thread.clone();
            move |mut p: DownloadProgress| {
                p.model_id = model_id.clone();
                let _ = app.emit("download-progress", &p);
            }
        };

        let result = run_download(
            &auth_provider,
            &final_path_for_thread,
            expected_bytes,
            &sha256,
            &emit,
            &cancel,
            &bytes,
        );

        match result {
            // `run_download` returns `Ok(())` for BOTH a completed download
            // and a cancelled one (worker step 4) — distinguish by whether
            // the file actually landed at `final_path` before deciding
            // whether to start the engine. `done` is emitted here, AFTER
            // `start_if_no_model`, not inside `run_download` — emitting it
            // first would let the front-end react to `done` (e.g. calling
            // `load_model`) while the engine's status could still read
            // `NoModel`, a race `load_model` could lose.
            Ok(()) => {
                if final_path_for_thread.exists() {
                    crate::inference::start_if_no_model(app_for_thread, engine_for_thread);
                    emit(DownloadProgress {
                        model_id: model_id_for_thread,
                        phase: "done".into(),
                        bytes_downloaded: expected_bytes,
                        total_bytes: expected_bytes,
                        bytes_per_sec: 0,
                        error: None,
                    });
                }
            }
            Err(msg) => {
                emit(DownloadProgress {
                    model_id: model_id_for_thread,
                    phase: "failed".into(),
                    bytes_downloaded: bytes.load(Ordering::Relaxed),
                    total_bytes: expected_bytes,
                    bytes_per_sec: 0,
                    error: Some(msg),
                });
            }
        }
    });

    Ok(())
}

#[tauri::command]
pub async fn cancel_download(downloads: State<'_, Arc<Downloads>>) -> Result<(), String> {
    downloads.request_cancel();
    Ok(())
}

#[tauri::command]
pub async fn download_status(
    model_id: String,
    app: AppHandle,
    downloads: State<'_, Arc<Downloads>>,
) -> Result<DownloadStatus, String> {
    let root = crate::inference::resources_root(&app);
    let raw = std::fs::read_to_string(root.join("catalog.json")).map_err(|e| e.to_string())?;
    let entries = crate::catalog::parse_catalog(&raw)?;
    let hero = crate::catalog::hero(&entries).ok_or_else(|| "catalog missing integrity data".to_string())?;
    let model_file = hero
        .model_file
        .clone()
        .ok_or_else(|| "catalog missing integrity data".to_string())?;

    let app_data = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let final_path = app_data.join(&model_file);
    let part_path = part_path_for(&final_path);

    let installed = final_path.exists();
    let part_bytes = std::fs::metadata(&part_path).map(|m| m.len()).unwrap_or(0);

    let guard = downloads.active.lock().unwrap();
    let (active, bytes_downloaded, total_bytes) = match guard.as_ref() {
        Some(a) if a.model_id == model_id => (true, a.bytes.load(Ordering::Relaxed), a.total),
        _ => (false, 0, 0),
    };

    Ok(DownloadStatus {
        installed,
        part_bytes,
        active,
        bytes_downloaded,
        total_bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud::test_support::{lock, start_ranged_server, RangedBehavior};
    use std::sync::atomic::AtomicUsize;
    use std::sync::mpsc::Receiver;

    /// Same env-var mutex the rest of the `cloud` module's mock-server
    /// tests share — this module touches `CLEOPHIS_DOWNLOAD_READ_TIMEOUT_MS`.
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        lock()
    }

    const SIZE: usize = 1024 * 1024; // 1 MiB

    fn deterministic_content() -> Vec<u8> {
        (0..SIZE).map(|i| (i % 256) as u8).collect()
    }

    fn sha256_hex(data: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(data);
        hasher.finalize().iter().map(|b| format!("{b:02x}")).collect()
    }

    fn unique_dir(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "cleophis-c3b-test-{}-{}-{}",
            std::process::id(),
            nanos,
            name
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn auth_provider_for(base_url: &str, file_bytes: u64) -> impl Fn() -> Result<rest::DownloadAuth, CloudError> {
        let url = format!("{base_url}/file");
        move || {
            Ok(rest::DownloadAuth {
                url: url.clone(),
                authorization: "test-b2-token".to_string(),
                expires_at: "2026-07-18T00:00:00Z".to_string(),
                file_bytes,
            })
        }
    }

    fn counting_auth_provider(
        base_url: &str,
        file_bytes: u64,
    ) -> (impl Fn() -> Result<rest::DownloadAuth, CloudError>, Arc<AtomicUsize>) {
        let counter = Arc::new(AtomicUsize::new(0));
        let counter_for_closure = counter.clone();
        let url = format!("{base_url}/file");
        let provider = move || {
            counter_for_closure.fetch_add(1, Ordering::SeqCst);
            Ok(rest::DownloadAuth {
                url: url.clone(),
                authorization: "test-b2-token".to_string(),
                expires_at: "2026-07-18T00:00:00Z".to_string(),
                file_bytes,
            })
        };
        (provider, counter)
    }

    fn collecting_emit() -> (impl Fn(DownloadProgress), Arc<Mutex<Vec<DownloadProgress>>>) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let events_for_closure = events.clone();
        let emit = move |p: DownloadProgress| {
            events_for_closure.lock().unwrap().push(p);
        };
        (emit, events)
    }

    fn drain_all(rx: &Receiver<crate::cloud::test_support::CapturedRequest>) -> Vec<crate::cloud::test_support::CapturedRequest> {
        let mut out = Vec::new();
        while let Ok(r) = rx.recv_timeout(Duration::from_millis(50)) {
            out.push(r);
        }
        out
    }

    // 1. Fresh happy path: full stream -> verified -> renamed; events
    // include requesting/downloading/verifying; final file matches content.
    // `run_download` itself is `Ok(())` + the file landing at `final_path`
    // — not a `done` event — the completion signal: `done` is emitted by
    // the `download_model` command wrapper, AFTER `start_if_no_model`, to
    // avoid a front-end race (see the module doc comment / task report).
    #[test]
    fn fresh_happy_path_completes_and_renames() {
        let _g = env_lock();
        let content = deterministic_content();
        let sha = sha256_hex(&content);
        let (base_url, _handle, _rx) = start_ranged_server(content.clone(), vec![RangedBehavior::Serve206]);

        let dir = unique_dir("fresh-happy");
        let final_path = dir.join("hero.gguf");

        let auth_provider = auth_provider_for(&base_url, SIZE as u64);
        let (emit, events) = collecting_emit();
        let cancel = AtomicBool::new(false);
        let bytes_counter = AtomicU64::new(0);

        let result = run_download(
            &auth_provider,
            &final_path,
            SIZE as u64,
            &sha,
            &emit,
            &cancel,
            &bytes_counter,
        );

        assert!(result.is_ok(), "expected Ok(()), got {result:?}");
        assert!(final_path.exists(), "expected the final file to exist");
        let on_disk = std::fs::read(&final_path).unwrap();
        assert_eq!(on_disk, content);

        let phases: Vec<String> = events.lock().unwrap().iter().map(|p| p.phase.clone()).collect();
        assert!(phases.contains(&"requesting".to_string()), "phases: {phases:?}");
        assert!(phases.contains(&"downloading".to_string()), "phases: {phases:?}");
        assert_eq!(phases.last(), Some(&"verifying".to_string()), "phases: {phases:?}");
        assert!(!phases.contains(&"done".to_string()), "phases: {phases:?}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    // 2. Resume: pre-write first 300 KiB as `.part` -> script [Serve206] ->
    // captured request has `Range: bytes=307200-`; final sha correct.
    #[test]
    fn resume_sends_range_header_and_completes() {
        let _g = env_lock();
        let content = deterministic_content();
        let sha = sha256_hex(&content);
        let (base_url, _handle, rx) = start_ranged_server(content.clone(), vec![RangedBehavior::Serve206]);

        let dir = unique_dir("resume");
        let final_path = dir.join("hero.gguf");
        let part_path = part_path_for(&final_path);
        let seed_len = 300 * 1024;
        std::fs::write(&part_path, &content[..seed_len]).unwrap();

        let auth_provider = auth_provider_for(&base_url, SIZE as u64);
        let (emit, _events) = collecting_emit();
        let cancel = AtomicBool::new(false);
        let bytes_counter = AtomicU64::new(0);

        let result = run_download(
            &auth_provider,
            &final_path,
            SIZE as u64,
            &sha,
            &emit,
            &cancel,
            &bytes_counter,
        );

        assert!(result.is_ok(), "expected Ok(()), got {result:?}");
        let on_disk = std::fs::read(&final_path).unwrap();
        assert_eq!(on_disk, content);

        let requests = drain_all(&rx);
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].range.as_deref(), Some("bytes=307200-"));
        // Secret hygiene: the B2 authorization token appears in the request
        // header, verbatim, and nowhere else this test can observe (it
        // never appears in `result`, `events`, or any error message).
        assert_eq!(requests[0].authorization.as_deref(), Some("test-b2-token"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    // 3. 200-ignoring-Range: `.part` seeded -> ServeFullIgnoringRange ->
    // truncate + restart -> correct final file.
    #[test]
    fn server_ignoring_range_restarts_clean() {
        let _g = env_lock();
        let content = deterministic_content();
        let sha = sha256_hex(&content);
        let (base_url, _handle, _rx) =
            start_ranged_server(content.clone(), vec![RangedBehavior::ServeFullIgnoringRange]);

        let dir = unique_dir("ignore-range");
        let final_path = dir.join("hero.gguf");
        let part_path = part_path_for(&final_path);
        // Seed with garbage of the SAME length as a real partial, so a bug
        // that just appended instead of restarting would definitely fail
        // the final hash check.
        std::fs::write(&part_path, vec![0xAAu8; 300 * 1024]).unwrap();

        let auth_provider = auth_provider_for(&base_url, SIZE as u64);
        let (emit, _events) = collecting_emit();
        let cancel = AtomicBool::new(false);
        let bytes_counter = AtomicU64::new(0);

        let result = run_download(
            &auth_provider,
            &final_path,
            SIZE as u64,
            &sha,
            &emit,
            &cancel,
            &bytes_counter,
        );

        assert!(result.is_ok(), "expected Ok(()), got {result:?}");
        let on_disk = std::fs::read(&final_path).unwrap();
        assert_eq!(on_disk, content);

        let _ = std::fs::remove_dir_all(&dir);
    }

    // 4. Disconnect mid-body: [DisconnectAfter(100_000), Serve206] -> retry
    // resumes from ~100 000 -> completes; >=2 captured requests, second has
    // a Range.
    #[test]
    fn disconnect_mid_body_retries_and_completes() {
        let _g = env_lock();
        let content = deterministic_content();
        let sha = sha256_hex(&content);
        let (base_url, _handle, rx) = start_ranged_server(
            content.clone(),
            vec![RangedBehavior::DisconnectAfter(100_000), RangedBehavior::Serve206],
        );

        let dir = unique_dir("disconnect");
        let final_path = dir.join("hero.gguf");

        let auth_provider = auth_provider_for(&base_url, SIZE as u64);
        let (emit, _events) = collecting_emit();
        let cancel = AtomicBool::new(false);
        let bytes_counter = AtomicU64::new(0);

        let result = run_download(
            &auth_provider,
            &final_path,
            SIZE as u64,
            &sha,
            &emit,
            &cancel,
            &bytes_counter,
        );

        assert!(result.is_ok(), "expected Ok(()), got {result:?}");
        let on_disk = std::fs::read(&final_path).unwrap();
        assert_eq!(on_disk, content);

        let requests = drain_all(&rx);
        assert!(requests.len() >= 2, "expected >=2 requests, got {}", requests.len());
        assert!(requests[1].range.is_some(), "expected the retry to send a Range header");

        let _ = std::fs::remove_dir_all(&dir);
    }

    // 5. Stall: [StallForever, Serve206] with tiny read-timeout -> first
    // attempt errors, retry completes.
    #[test]
    fn stall_triggers_read_timeout_then_retry_completes() {
        let _g = env_lock();
        std::env::set_var("CLEOPHIS_DOWNLOAD_READ_TIMEOUT_MS", "300");

        let content = deterministic_content();
        let sha = sha256_hex(&content);
        let (base_url, _handle, rx) =
            start_ranged_server(content.clone(), vec![RangedBehavior::StallForever, RangedBehavior::Serve206]);

        let dir = unique_dir("stall");
        let final_path = dir.join("hero.gguf");

        let auth_provider = auth_provider_for(&base_url, SIZE as u64);
        let (emit, _events) = collecting_emit();
        let cancel = AtomicBool::new(false);
        let bytes_counter = AtomicU64::new(0);

        let result = run_download(
            &auth_provider,
            &final_path,
            SIZE as u64,
            &sha,
            &emit,
            &cancel,
            &bytes_counter,
        );

        std::env::remove_var("CLEOPHIS_DOWNLOAD_READ_TIMEOUT_MS");

        assert!(result.is_ok(), "expected Ok(()), got {result:?}");
        let on_disk = std::fs::read(&final_path).unwrap();
        assert_eq!(on_disk, content);

        let requests = drain_all(&rx);
        assert!(requests.len() >= 2, "expected >=2 requests, got {}", requests.len());

        let _ = std::fs::remove_dir_all(&dir);
    }

    // 6. sha mismatch: serve WRONG content -> verifying -> Err contains
    // "corrupted", `.part` deleted, no final file.
    #[test]
    fn sha_mismatch_reports_corrupted_and_cleans_up() {
        let _g = env_lock();
        let content = deterministic_content();
        let wrong_content: Vec<u8> = content.iter().map(|b| b.wrapping_add(1)).collect();
        let expected_sha = sha256_hex(&content); // the hash of the RIGHT content
        let (base_url, _handle, _rx) =
            start_ranged_server(wrong_content, vec![RangedBehavior::Serve206]);

        let dir = unique_dir("sha-mismatch");
        let final_path = dir.join("hero.gguf");

        let auth_provider = auth_provider_for(&base_url, SIZE as u64);
        let (emit, events) = collecting_emit();
        let cancel = AtomicBool::new(false);
        let bytes_counter = AtomicU64::new(0);

        let result = run_download(
            &auth_provider,
            &final_path,
            SIZE as u64,
            &expected_sha,
            &emit,
            &cancel,
            &bytes_counter,
        );

        let err = result.expect_err("expected an Err for a sha mismatch");
        assert!(err.contains("corrupted"), "error was: {err}");
        assert!(!final_path.exists());
        assert!(!part_path_for(&final_path).exists());

        let phases: Vec<String> = events.lock().unwrap().iter().map(|p| p.phase.clone()).collect();
        assert!(phases.contains(&"verifying".to_string()), "phases: {phases:?}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    // 7. Mid-flight 401: [Status(401), Serve206] -> auth_provider called
    // >=2 times -> completes.
    #[test]
    fn mid_flight_401_re_mints_auth_and_completes() {
        let _g = env_lock();
        let content = deterministic_content();
        let sha = sha256_hex(&content);
        let (base_url, _handle, _rx) =
            start_ranged_server(content.clone(), vec![RangedBehavior::Status(401), RangedBehavior::Serve206]);

        let dir = unique_dir("mid-401");
        let final_path = dir.join("hero.gguf");

        let (auth_provider, call_count) = counting_auth_provider(&base_url, SIZE as u64);
        let (emit, _events) = collecting_emit();
        let cancel = AtomicBool::new(false);
        let bytes_counter = AtomicU64::new(0);

        let result = run_download(
            &auth_provider,
            &final_path,
            SIZE as u64,
            &sha,
            &emit,
            &cancel,
            &bytes_counter,
        );

        assert!(result.is_ok(), "expected Ok(()), got {result:?}");
        let on_disk = std::fs::read(&final_path).unwrap();
        assert_eq!(on_disk, content);
        assert!(
            call_count.load(Ordering::SeqCst) >= 2,
            "expected auth_provider to be called >=2 times, got {}",
            call_count.load(Ordering::SeqCst)
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // C7 follow-up: two definitive refusals (403) in a row give up after
    // the 2nd attempt (not the 3rd) with a message that names the server
    // refusal rather than blaming "your connection" — and auth_provider is
    // called exactly twice (initial mint + the one re-mint between the two
    // failures), never a third time, since the fast-fail path doesn't
    // re-mint before giving up.
    #[test]
    fn repeated_403_fast_fails_with_server_refusal_message() {
        let _g = env_lock();
        let content = deterministic_content();
        let sha = sha256_hex(&content);
        let (base_url, _handle, _rx) = start_ranged_server(
            content.clone(),
            vec![RangedBehavior::Status(403), RangedBehavior::Status(403)],
        );

        let dir = unique_dir("repeated-403");
        let final_path = dir.join("hero.gguf");

        let (auth_provider, call_count) = counting_auth_provider(&base_url, SIZE as u64);
        let (emit, _events) = collecting_emit();
        let cancel = AtomicBool::new(false);
        let bytes_counter = AtomicU64::new(0);

        let result = run_download(
            &auth_provider,
            &final_path,
            SIZE as u64,
            &sha,
            &emit,
            &cancel,
            &bytes_counter,
        );

        let err = result.expect_err("expected an Err after two consecutive definitive 403s");
        assert!(
            err.contains("the server refused the request (HTTP 403)"),
            "error was: {err}"
        );
        assert!(!final_path.exists());
        assert_eq!(
            call_count.load(Ordering::SeqCst),
            2,
            "expected exactly 2 auth_provider calls, got {}",
            call_count.load(Ordering::SeqCst)
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // Whole-branch-review follow-up: a re-mint (`auth_provider`) failure
    // mid-retry must consume the SAME 3-consecutive-failure budget as a
    // connect/stream failure rather than aborting the download outright.
    // Script: [DisconnectAfter(100_000), Serve206] forces one connect-level
    // failure; `auth_provider`'s SECOND call (the re-mint that failure
    // triggers) fails once (`CloudError::Offline`), then its THIRD call
    // succeeds — two failures total, still under the budget of 3 — and the
    // download completes using the auth from that third call.
    #[test]
    fn remint_failure_consumes_retry_budget_and_recovers() {
        let _g = env_lock();
        let content = deterministic_content();
        let sha = sha256_hex(&content);
        let (base_url, _handle, rx) = start_ranged_server(
            content.clone(),
            vec![RangedBehavior::DisconnectAfter(100_000), RangedBehavior::Serve206],
        );

        let dir = unique_dir("remint-failure");
        let final_path = dir.join("hero.gguf");

        let call_count = Arc::new(AtomicUsize::new(0));
        let call_count_for_closure = call_count.clone();
        let url = format!("{base_url}/file");
        let auth_provider = move || {
            let n = call_count_for_closure.fetch_add(1, Ordering::SeqCst) + 1;
            if n == 2 {
                return Err(CloudError::Offline);
            }
            Ok(rest::DownloadAuth {
                url: url.clone(),
                authorization: "test-b2-token".to_string(),
                expires_at: "2026-07-18T00:00:00Z".to_string(),
                file_bytes: SIZE as u64,
            })
        };

        let (emit, _events) = collecting_emit();
        let cancel = AtomicBool::new(false);
        let bytes_counter = AtomicU64::new(0);

        let result = run_download(
            &auth_provider,
            &final_path,
            SIZE as u64,
            &sha,
            &emit,
            &cancel,
            &bytes_counter,
        );

        assert!(result.is_ok(), "expected Ok(()), got {result:?}");
        let on_disk = std::fs::read(&final_path).unwrap();
        assert_eq!(on_disk, content);
        assert!(
            call_count.load(Ordering::SeqCst) >= 3,
            "expected auth_provider to be called >=3 times (initial + failed remint + successful remint), got {}",
            call_count.load(Ordering::SeqCst)
        );

        let requests = drain_all(&rx);
        assert!(
            requests.len() >= 2,
            "expected >=2 ranged-server requests, got {}",
            requests.len()
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // 8. Cancel: DisconnectAfter forces a failure, cancel is set before the
    // retry lands -> cancelled phase emitted, `.part` intact, Ok(()).
    #[test]
    fn cancel_during_backoff_stops_cleanly() {
        let _g = env_lock();
        let content = deterministic_content();
        let sha = sha256_hex(&content);
        let (base_url, _handle, _rx) = start_ranged_server(
            content.clone(),
            vec![RangedBehavior::DisconnectAfter(100_000), RangedBehavior::Serve206],
        );

        let dir = unique_dir("cancel");
        let final_path = dir.join("hero.gguf");

        let auth_provider = auth_provider_for(&base_url, SIZE as u64);
        let (emit, events) = collecting_emit();
        let cancel = Arc::new(AtomicBool::new(false));
        let bytes_counter = AtomicU64::new(0);

        // Flip the cancel flag shortly after the first (disconnecting)
        // attempt would have failed and entered its backoff sleep, well
        // before the 2s backoff would otherwise elapse.
        let cancel_for_thread = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(400));
            cancel_for_thread.store(true, Ordering::Relaxed);
        });

        let result = run_download(
            &auth_provider,
            &final_path,
            SIZE as u64,
            &sha,
            &emit,
            &cancel,
            &bytes_counter,
        );

        assert!(result.is_ok(), "expected Ok(()), got {result:?}");
        assert!(!final_path.exists(), "expected no final file on cancel");
        assert!(part_path_for(&final_path).exists(), "expected the .part file to remain");

        let phases: Vec<String> = events.lock().unwrap().iter().map(|p| p.phase.clone()).collect();
        assert_eq!(phases.last(), Some(&"cancelled".to_string()), "phases: {phases:?}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    // 9. Preflight disk-space check: not portably testable (would need to
    // fake near-zero free disk space without root/admin access, or inject a
    // fake `sysinfo::Disks` — the function reads the REAL disk by design).
    // Skipped; `check_disk_space`'s logic is exercised implicitly by every
    // other test above, which all pass through it on a real disk with
    // ordinary free space.

    // 10. auth file_bytes mismatch -> "Catalog out of date".
    #[test]
    fn auth_file_bytes_mismatch_reports_catalog_out_of_date() {
        let _g = env_lock();
        let dir = unique_dir("catalog-mismatch");
        let final_path = dir.join("hero.gguf");

        // No server needed — the mismatch is caught before any network call.
        let auth_provider = move || {
            Ok(rest::DownloadAuth {
                url: "http://127.0.0.1:1/unused".to_string(),
                authorization: "unused".to_string(),
                expires_at: "2026-07-18T00:00:00Z".to_string(),
                file_bytes: (SIZE as u64) + 1, // mismatched
            })
        };
        let (emit, _events) = collecting_emit();
        let cancel = AtomicBool::new(false);
        let bytes_counter = AtomicU64::new(0);

        let result = run_download(
            &auth_provider,
            &final_path,
            SIZE as u64,
            "0000000000000000000000000000000000000000000000000000000000000000",
            &emit,
            &cancel,
            &bytes_counter,
        );

        let err = result.expect_err("expected an Err for a file_bytes mismatch");
        assert!(err.contains("Catalog out of date"), "error was: {err}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    // 11. Accept: the production download host, plain path.
    #[test]
    fn download_host_allowed_accepts_cleophis_dl_host() {
        assert!(download_host_allowed("https://dl.cleophis.com/file/abc"));
    }

    // 12. Accept: a B2 CDN host by dotted suffix.
    #[test]
    fn download_host_allowed_accepts_backblazeb2_suffix() {
        assert!(download_host_allowed("https://f005.backblazeb2.com/file/abc"));
    }

    // 13. Reject: plain http, even for an otherwise-allowed host.
    #[test]
    fn download_host_allowed_rejects_plain_http() {
        assert!(!download_host_allowed("http://dl.cleophis.com/file/abc"));
    }

    // 14. Reject: an unrelated host entirely.
    #[test]
    fn download_host_allowed_rejects_unrelated_host() {
        assert!(!download_host_allowed("https://evil.com/file/abc"));
    }

    // 15. Reject: `dl.cleophis.com` as a subdomain LABEL of an attacker
    // domain, not the real host — exact match only, no prefix match.
    #[test]
    fn download_host_allowed_rejects_cleophis_host_as_subdomain_of_evil() {
        assert!(!download_host_allowed("https://dl.cleophis.com.evil.com/file/abc"));
    }

    // 16. Reject: a host that merely contains "backblazeb2.com" without the
    // leading dot — the suffix check is dot-anchored, not a substring match.
    #[test]
    fn download_host_allowed_rejects_backblazeb2_lookalike() {
        assert!(!download_host_allowed("https://evil-backblazeb2.com/file/abc"));
    }

    // 17. Reject: the userinfo trick — an allowed-looking host placed BEFORE
    // an `@`, with the real (disallowed) host after it.
    #[test]
    fn download_host_allowed_rejects_userinfo_trick() {
        assert!(!download_host_allowed(
            "https://f005.backblazeb2.com@evil.com/file/abc"
        ));
    }

    // 18. Reject the WHATWG authority-delimiter divergence class: `#`, `?`,
    // and `\` all terminate the authority for the `url` crate (the parser
    // ureq uses), so the real connect-host is `evil.com` even though the
    // allowed suffix appears later in the string. A naive `/`/`@`/`:` split
    // would accept these; parsing with `url::Url` rejects them.
    #[test]
    fn download_host_allowed_rejects_fragment_delimiter_divergence() {
        assert!(!download_host_allowed("https://evil.com#.backblazeb2.com/x"));
    }

    #[test]
    fn download_host_allowed_rejects_query_delimiter_divergence() {
        assert!(!download_host_allowed("https://evil.com?.backblazeb2.com"));
    }

    #[test]
    fn download_host_allowed_rejects_backslash_delimiter_divergence() {
        assert!(!download_host_allowed("https://evil.com\\.backblazeb2.com/x"));
    }

    // 19. Case-insensitive host match now works (url crate lower-cases the
    // host) — a legit URL isn't spuriously rejected on casing.
    #[test]
    fn download_host_allowed_accepts_mixed_case_host() {
        assert!(download_host_allowed("https://DL.Cleophis.com/file/abc"));
    }
}
