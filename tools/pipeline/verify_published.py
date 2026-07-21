#!/usr/bin/env python3
"""verify_published.py — Task A6: consumer self-check from the PUBLIC path.

This is the publish's definition of done: it impersonates the shipped app
exactly, using ONLY what the app itself has — a base URL and a pinned
public ed25519 key, no credentials of any kind — and performs the exact
same sequence the app performs (see `src-tauri/src/catalog_dist.rs`'s
`fetch_and_verify_catalog`/`parse_and_verify` for the catalog half, and
`src-tauri/src/cloud/download.rs`'s public-artifact download path for the
per-artifact half): GET `catalog.json` + `catalog.json.sig`, verify the
detached ed25519 signature over the EXACT fetched catalog bytes, parse +
sanity-check the catalog, then for every listed artifact GET it and verify
its streamed sha256 + size against what the signed catalog claims. ANY
failure — HTTP, signature, schema, hash, size, malformed signature length —
is a one-line `FAIL: ...` naming exactly what went wrong, and a non-zero
exit. Only if every artifact verifies does it print `OK: ...` and exit 0.

Env-dumb (see README.md "The env-dumb contract") in the strictest possible
sense: this script's ONLY inputs are its CLI flags below. It never reads
`tools/pipeline/.env` (not even to look) and never touches boto3/B2
credentials — nothing here needs `.env` to exist at all. This is
deliberate: a bad actor with only the public bucket URL and the published
public key should be able to run the exact same check a real user's app
performs, with zero secrets, from any machine.

Inputs:
    --base-url <url>   Public artifact base URL (default:
                        https://cleophis-dist.s3.us-east-005.backblazeb2.com —
                        the same compiled-in constant the app pins as
                        `catalog_dist::ARTIFACT_BASE_URL`). `catalog.json`,
                        `catalog.json.sig`, and every artifact `path` in
                        the catalog are resolved relative to this base.
    --pubkey <hex>      REQUIRED unless --self-test. 64-hex-char ed25519
                        curator PUBLIC key. No default, no compiled-in
                        key — the caller must supply exactly the key the
                        app itself is pinned to (at handoff, the production
                        curator public key), so this script can never
                        silently "verify" against the wrong key.
    --self-test         Spins up local-only stub HTTP servers (stdlib
                        http.server, no real network) plus a throwaway,
                        in-memory-only ed25519 keypair, and actually
                        SPAWNS this script as a subprocess against each —
                        exercising the real CLI end to end (argument
                        parsing, urllib, retry logic, exit code), not just
                        its internal functions. Never touches --base-url,
                        --pubkey, or any real bucket. See self_test()'s
                        docstring for the exact scenarios covered.

Outputs: none on disk. Everything is stdout/stderr and the process exit
code. Progress ([fetch]/[verify] lines) and the final `OK: ...` line go to
stdout; every `FAIL: ...`/`error: ...` line goes to stderr.

Sequence (mirrors the app exactly — see the module docstring above):
    1. GET <base-url>/catalog.json, capped at 1 MiB.
    2. GET <base-url>/catalog.json.sig — must be EXACTLY 64 bytes (a raw
       detached ed25519 signature always is; any other length is a clean
       refusal, never "close enough").
    3. `sign_catalog.verify_bytes(pubkey, catalog_bytes, sig_bytes)` over
       the EXACT fetched catalog bytes (never a re-serialization) — the
       one crypto path this pipeline has, reused verbatim per the task
       brief ("reuse the Python verify from A4"), the Python-side twin of
       `kpack_core::sign::verify_detached` the app itself calls.
    4. Parse JSON, then sanity-check: `catalog_version` is an int in
       [0, 1_000_000); `artifacts` is non-empty; every artifact carries
       all 7 wire-contract fields (path, sha256, size, kind, base_model,
       version, license — see README.md's "Binding cross-track values"
       table) with `kind` in {base, adapter} and `base_model` one of
       the known tiers (Qwen3-4B, Llama-3.2-1B, Qwen3-8B).
    5. For EACH artifact: GET <base-url>/<path> — refusing (RedirectError)
       rather than following any 3xx response, exactly like the app's own
       `download.rs::streaming_agent()`, which pins `.redirects(0)`
       because a public/signed artifact URL never legitimately redirects
       — streaming the response body through sha256 in 1 MiB chunks while
       counting bytes — never written to disk, never buffered whole in
       memory, regardless of artifact size (these are multi-GB GGUF
       files) — then compare the resulting sha256 (case-insensitive) and
       byte count against the catalog's recorded values for that
       artifact. Size is compared before sha256 (fails fast, and is what
       lets a truncated-artifact self-test scenario report distinctly
       from a tampered-byte one). The catalog.json/.sig fetches in steps
       1-2 deliberately do NOT refuse redirects — they keep urllib's
       default redirect-following behavior, matching the app's own
       catalog fetch (`catalog_dist.rs::get_capped`), which pins no
       redirect policy either. Refusing artifact redirects specifically
       (not catalog redirects) is what makes this script a faithful
       fidelity gate: a redirecting bucket/CDN in front of an artifact
       must make this script FAIL the same way the shipped app would,
       never print OK while the app itself refuses the download.

Retry policy: every GET (catalog, sig, and each artifact) gets a bounded
30-second per-operation timeout (covers both connect and each individual
read on the socket) and up to 3 attempts with a 5s/10s backoff — but ONLY
for transient failures: a 5xx HTTP status or a network-level
(`urllib.error.URLError`, e.g. connection refused/DNS failure/timeout)
error. A 4xx (including 404) is never retried — mirrors publish.py's
`S3Client`/`_retry`'s "never retry a definitive not-found" stance, just
generalized to "any definitive 4xx". A 3xx on an artifact URL (see step 5
above) is likewise never retried — RedirectError is deliberately not a
FetchError subclass, so it structurally never enters the retry loop at
all, same reasoning as a 4xx: retrying would just observe the same
redirect again. Signature/hash/size/schema verification failures are
computed AFTER a fetch already succeeded, so they too are structurally
never inside the retry loop — a bad hash can never be retried into
passing; retrying more GETs of already-correctly-received bytes wouldn't
change the comparison's outcome anyway.

Documented invocations:

    Verify the live production bucket (post-handoff, once the production
    curator public key is known — Step 2 of this task per the brief; must
    be green before the app wrapper is pointed at cleophis-dist):
        ./.venv/bin/python3 verify_published.py --pubkey <64-hex production pubkey>

    Self-test (no network to any real bucket, no credentials, fully
    runnable pre-handoff):
        ./.venv/bin/python3 verify_published.py --self-test

Importable API: none of this script's functions are imported elsewhere in
the pipeline (unlike sign_catalog.py's verify_bytes/sign_bytes) — this is
the last, standalone consumer-facing link in the chain, not a library other
pipeline scripts build on.
"""

from __future__ import annotations

import argparse
import hashlib
import http.server
import json
import socket
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request
from pathlib import Path
from typing import Callable, TypeVar

from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
from cryptography.hazmat.primitives.serialization import Encoding, NoEncryption, PrivateFormat, PublicFormat

from sign_catalog import sign_bytes, verify_bytes  # Task A6 binding decision — reuse this one crypto path.

PIPELINE_ROOT = Path(__file__).resolve().parent

DEFAULT_BASE_URL = "https://cleophis-dist.s3.us-east-005.backblazeb2.com"

MAX_CATALOG_BYTES = 1024 * 1024  # 1 MiB — mirrors sign_catalog.py/publish.py/the app's own catalog.json cap.
SIG_BYTES_LEN = 64  # a raw detached ed25519 signature always is — see kpack-core's read_sig_capped.
SIG_READ_CAP = 4096  # generous infra cap on a fetched .sig; the exact-64 check is the business rule (mirrors publish.py).
HASH_CHUNK_SIZE = 1024 * 1024  # 1 MiB streaming chunks — an artifact is never buffered whole, regardless of size.

REQUEST_TIMEOUT = 30  # seconds; socket-level, covers both connect and each individual read.
MAX_ATTEMPTS = 3
RETRY_BACKOFF_BASE = 5  # seconds; attempt N waits N * this many seconds (mirrors publish.py's S3Client._retry).

MAX_SANE_CATALOG_VERSION = 1_000_000  # matches build_catalog.py/publish.py's own hostile-version bound.
REQUIRED_ARTIFACT_FIELDS = ("path", "sha256", "size", "kind", "base_model", "version", "license")
VALID_KINDS = {"base", "adapter"}
KNOWN_BASE_MODELS = frozenset({"Qwen3-4B", "Llama-3.2-1B", "Qwen3-8B"})

# HTTP redirect statuses urllib's HTTPRedirectHandler intercepts (301/302/
# 303/307 -- see its http_error_30x aliases) plus 308 for completeness
# (not handled by this stdlib's HTTPRedirectHandler at all, so it always
# surfaces as a plain HTTPError regardless of any redirect policy — still
# worth classifying as a redirect here rather than a generic FetchError).
REDIRECT_STATUS_CODES = {301, 302, 303, 307, 308}

T = TypeVar("T")


# ---------------------------------------------------------------------
# Failure taxonomy — every one of these is caught at main()'s top level
# and reported as a single `FAIL: <str(exc)>` line, never a raw traceback.
# Kept as small, distinct classes (rather than one generic exception) so
# main()/self_test() can tell categories apart without string-sniffing.
# ---------------------------------------------------------------------


class VerifyFailure(Exception):
    """Base class for every clean, reportable FAIL this script can raise."""


class FetchError(VerifyFailure):
    """An HTTP GET did not succeed after exhausting retries (or failed on
    a non-retryable status). `status` is the HTTP status code if the
    server actually responded with one (e.g. 404), else None for a
    network-level failure (connection refused, DNS failure, timeout)."""

    def __init__(self, url: str, status: int | None = None, reason: str | None = None):
        self.url = url
        self.status = status
        self.reason = reason
        if status is not None:
            message = f"HTTP {status} fetching {url}"
        else:
            message = f"network error fetching {url}: {reason}"
        super().__init__(message)


class ResponseTooLarge(VerifyFailure):
    def __init__(self, url: str, max_bytes: int):
        super().__init__(f"response from {url} exceeded the {max_bytes}-byte sanity cap")


class RedirectError(VerifyFailure):
    """An ARTIFACT URL responded with a 3xx redirect. The app's own
    artifact download agent (`download.rs::streaming_agent`) pins
    `.redirects(0)` — a public/signed artifact URL never legitimately
    redirects, so the app treats any 3xx there as a hard failure rather
    than transparently following it. This script must fail the exact same
    way: a redirecting bucket/CDN must never make this gate print OK while
    the shipped app refuses the download — that's precisely the "gate
    passes, app fails" outcome this script exists to catch. Deliberately
    NOT a subclass of FetchError, so it can never be picked up by
    `_fetch_with_retry`'s retry loop — a redirect is exactly as definitive
    an answer as a 4xx, never worth retrying. catalog.json/.sig fetches
    never raise this (they keep urllib's default, redirect-following
    behavior — see fetch_capped — matching the app's own un-pinned
    catalog_dist.rs::get_capped)."""

    def __init__(self, url: str, status: int, location: str | None):
        self.url = url
        self.status = status
        self.location = location
        message = f"artifact URL redirected (HTTP {status}) — the app refuses redirects: {url}"
        if location:
            message += f" -> {location}"
        super().__init__(message)


class SigLengthError(VerifyFailure):
    def __init__(self, url: str, actual_len: int):
        super().__init__(f"{url} is {actual_len} bytes, expected exactly {SIG_BYTES_LEN} (a raw detached ed25519 signature)")


class SignatureError(VerifyFailure):
    def __init__(self):
        super().__init__("catalog.json.sig does NOT verify against catalog.json under the given --pubkey")


class SchemaError(VerifyFailure):
    """catalog.json parsed as JSON but violates the wire-format contract
    (see README.md's "Binding cross-track values" table) — malformed JSON
    itself is also reported through this class."""


class Sha256Mismatch(VerifyFailure):
    def __init__(self, path: str, expected: str, actual: str):
        self.path = path
        super().__init__(f"sha256 mismatch for {path}: catalog says {expected}, got {actual}")


class SizeMismatch(VerifyFailure):
    def __init__(self, path: str, expected: int, actual: int):
        self.path = path
        super().__init__(f"size mismatch for {path}: catalog says {expected} bytes, got {actual} bytes")


# ---------------------------------------------------------------------
# HTTP fetch — stdlib urllib.request only (no boto3, no requests, no
# .env): this must run on any machine that has just the pubkey. Bounded
# timeout + bounded retry on transient failures only, per the module
# docstring's "Retry policy" section.
# ---------------------------------------------------------------------


class _NoRedirectHandler(urllib.request.HTTPRedirectHandler):
    """Installed ONLY on `_ARTIFACT_OPENER` (see below), used ONLY by
    fetch_and_hash_streaming's artifact GETs — never by fetch_capped's
    catalog/.sig GETs. Overriding `redirect_request` to always return None
    tells the (otherwise unmodified) inherited `http_error_30x` methods
    "don't follow this" — per CPython's urllib.request source, when
    `redirect_request` returns None those methods return None too, which
    makes `OpenerDirector.error()` fall through to the default handler and
    raise a plain `HTTPError` with `.code` set to the 3xx status and
    `.headers` carrying `Location`. That HTTPError is then translated into
    a RedirectError by `_urlopen` below, exactly like a 4xx/5xx would be
    translated into a FetchError. This mirrors `download.rs::
    streaming_agent()`'s `.redirects(0)` exactly, at the Python layer."""

    def redirect_request(self, req, fp, code, msg, headers, newurl):  # noqa: N802 - stdlib-mandated signature
        return None


_ARTIFACT_OPENER = urllib.request.build_opener(_NoRedirectHandler)


def _urlopen(url: str, timeout: int = REQUEST_TIMEOUT, *, opener: urllib.request.OpenerDirector | None = None):
    """GETs `url`, converting every failure mode into a `FetchError` (or,
    for a 3xx encountered through `opener=_ARTIFACT_OPENER`, a
    `RedirectError`) — never a raw urllib exception escaping this
    function. `opener=None` (the default, used by fetch_capped for
    catalog.json/.sig) is plain `urllib.request.urlopen`, which follows
    redirects per urllib's normal default — matching the app's own
    catalog fetch, which pins no redirect policy either."""
    request = urllib.request.Request(url, method="GET")
    open_fn = opener.open if opener is not None else urllib.request.urlopen
    try:
        return open_fn(request, timeout=timeout)  # noqa: S310 - public HTTP(S) fetch is the whole point
    except urllib.error.HTTPError as exc:
        if exc.code in REDIRECT_STATUS_CODES:
            location = exc.headers.get("Location") if exc.headers is not None else None
            raise RedirectError(url, exc.code, location) from exc
        raise FetchError(url, status=exc.code) from exc
    except urllib.error.URLError as exc:
        raise FetchError(url, reason=str(exc.reason)) from exc
    except socket.timeout as exc:
        raise FetchError(url, reason=f"timed out after {timeout}s") from exc


def _is_retryable(exc: Exception) -> bool:
    """True only for a 5xx HTTP status or a network-level (no status at
    all) failure — see the module docstring's "Retry policy" section. A
    4xx (including 404) is a definitive answer from the server and is
    never retried, same stance as publish.py's S3Client not retrying a
    definitive not-found."""
    if not isinstance(exc, FetchError):
        return False
    return exc.status is None or 500 <= exc.status < 600


def _fetch_with_retry(description: str, attempt_fn: Callable[[], T], max_attempts: int = MAX_ATTEMPTS) -> T:
    last_exc: Exception | None = None
    for attempt in range(1, max_attempts + 1):
        try:
            return attempt_fn()
        except FetchError as exc:
            last_exc = exc
            if not _is_retryable(exc) or attempt == max_attempts:
                raise
            delay = RETRY_BACKOFF_BASE * attempt
            print(f"[fetch] {description} attempt {attempt}/{max_attempts} failed: {exc} — retrying in {delay}s", flush=True)
            time.sleep(delay)
    raise last_exc  # pragma: no cover - unreachable: the loop above always returns or raises


def fetch_capped(url: str, max_bytes: int, *, max_attempts: int = MAX_ATTEMPTS) -> bytes:
    """GETs `url`, reading at most `max_bytes + 1` bytes so an oversized
    response is caught without ever buffering more than that — used for
    catalog.json and catalog.json.sig, both small, known-bounded files.
    Never used for artifacts (see fetch_and_hash_streaming below)."""

    def one_attempt() -> bytes:
        response = _urlopen(url)
        try:
            data = response.read(max_bytes + 1)
        except (urllib.error.URLError, socket.timeout, OSError) as exc:
            raise FetchError(url, reason=str(exc)) from exc
        finally:
            response.close()
        if len(data) > max_bytes:
            raise ResponseTooLarge(url, max_bytes)
        return data

    return _fetch_with_retry(f"GET {url}", one_attempt, max_attempts)


def fetch_and_hash_streaming(url: str, expected_size: int, *, max_attempts: int = MAX_ATTEMPTS) -> tuple[str, int]:
    """GETs `url`, streaming the response body through sha256 in
    HASH_CHUNK_SIZE (1 MiB) chunks. Never writes to disk, never holds more
    than the current chunk + running hash state in memory, regardless of
    the artifact's actual size (these are multi-GB GGUF files). Returns
    (sha256_hex, actual_byte_count); the caller compares both against the
    catalog's recorded values — this function itself never raises on a
    mismatch, only on an actual fetch failure.

    Stops reading (without raising — the caller's size comparison reports
    it) as soon as more bytes than `expected_size` have been seen, so a
    runaway or hostile response body is never streamed indefinitely just
    to re-prove a size mismatch that's already established.

    A mid-stream failure retries the WHOLE fetch from the start (no
    resume/range machinery) — mirrors catalog_dist.rs's own get_capped,
    which is deliberately non-resuming for the same reason: simple beats
    resume-complexity for what amounts to a bounded number of files in one
    verification run.

    Uses `_ARTIFACT_OPENER` (redirects refused, never followed) — mirrors
    `download.rs::streaming_agent()`'s `.redirects(0)`: a public/signed
    artifact URL never legitimately redirects, and the shipped app hard-
    fails on any 3xx there, so this script must too (see RedirectError).
    This is deliberately NOT applied to fetch_capped's catalog.json/.sig
    GETs, which keep urllib's default redirect-following behavior to match
    the app's own un-pinned catalog fetch.
    """

    def one_attempt() -> tuple[str, int]:
        response = _urlopen(url, opener=_ARTIFACT_OPENER)
        digest = hashlib.sha256()
        total = 0
        try:
            while True:
                try:
                    chunk = response.read(HASH_CHUNK_SIZE)
                except (urllib.error.URLError, socket.timeout, OSError) as exc:
                    raise FetchError(url, reason=str(exc)) from exc
                if not chunk:
                    break
                digest.update(chunk)
                total += len(chunk)
                if total > expected_size:
                    break  # already provably a size mismatch; stop pulling bytes off the wire
        finally:
            response.close()
        return digest.hexdigest(), total

    return _fetch_with_retry(f"GET {url}", one_attempt, max_attempts)


# ---------------------------------------------------------------------
# Catalog schema sanity check (step 4 of the sequence).
# ---------------------------------------------------------------------


def validate_catalog_schema(parsed: object) -> dict:
    """Raises SchemaError on any wire-format violation; returns `parsed`
    (narrowed to dict) unchanged on success. See the module docstring's
    step 4 for exactly what's checked — this is deliberately the same
    shape of check publish.py's validate_artifact_entry/load_catalog
    perform, just from the consumer side."""
    if not isinstance(parsed, dict):
        raise SchemaError("catalog.json does not contain a JSON object")

    version = parsed.get("catalog_version")
    if not isinstance(version, int) or isinstance(version, bool):
        raise SchemaError("catalog.json has no integer 'catalog_version' field")
    if not (0 <= version < MAX_SANE_CATALOG_VERSION):
        raise SchemaError(f"catalog.json catalog_version={version} is outside the sane bound [0, {MAX_SANE_CATALOG_VERSION})")

    artifacts = parsed.get("artifacts")
    if not isinstance(artifacts, list) or not artifacts:
        raise SchemaError("catalog.json has no non-empty 'artifacts' list")

    for index, artifact in enumerate(artifacts):
        if not isinstance(artifact, dict):
            raise SchemaError(f"catalog.json artifacts[{index}] is not a JSON object")
        missing = [field for field in REQUIRED_ARTIFACT_FIELDS if field not in artifact]
        if missing:
            raise SchemaError(f"catalog.json artifacts[{index}] is missing field(s) {missing}")

        kind = artifact["kind"]
        if kind not in VALID_KINDS:
            raise SchemaError(f"catalog.json artifacts[{index}] has kind={kind!r}, expected one of {sorted(VALID_KINDS)}")

        base_model = artifact["base_model"]
        if base_model not in KNOWN_BASE_MODELS:
            raise SchemaError(f"catalog.json artifacts[{index}] has base_model={base_model!r}, not one of the known tiers {sorted(KNOWN_BASE_MODELS)}")

        sha256 = artifact["sha256"]
        if not (isinstance(sha256, str) and len(sha256) == 64 and all(c in "0123456789abcdefABCDEF" for c in sha256)):
            raise SchemaError(f"catalog.json artifacts[{index}] sha256={sha256!r} is not a 64-hex-char digest")

        size = artifact["size"]
        if not isinstance(size, int) or isinstance(size, bool) or size <= 0:
            raise SchemaError(f"catalog.json artifacts[{index}] size={size!r} is not a positive integer")

        path = artifact["path"]
        if not isinstance(path, str) or not path:
            raise SchemaError(f"catalog.json artifacts[{index}] path={path!r} is not a non-empty string")

    return parsed


# ---------------------------------------------------------------------
# The verification sequence itself (steps 1-5 of the module docstring).
# ---------------------------------------------------------------------


def run_verify(base_url: str, pubkey_hex: str, *, max_attempts: int = MAX_ATTEMPTS) -> tuple[int, int]:
    """Runs the full public-path verification sequence against
    `base_url`. Raises a VerifyFailure subclass on the first thing that
    doesn't check out; returns (catalog_version, artifact_count) if and
    only if EVERY artifact verified. This is the one function both main()
    and every --self-test scenario ultimately exercise (self-test through
    the actual subprocess CLI, not by calling this directly — see
    self_test()'s docstring for why)."""
    catalog_url = f"{base_url}/catalog.json"
    sig_url = f"{base_url}/catalog.json.sig"

    catalog_bytes = fetch_capped(catalog_url, MAX_CATALOG_BYTES, max_attempts=max_attempts)
    sig_bytes = fetch_capped(sig_url, SIG_READ_CAP, max_attempts=max_attempts)

    if len(sig_bytes) != SIG_BYTES_LEN:
        raise SigLengthError(sig_url, len(sig_bytes))

    if not verify_bytes(pubkey_hex, catalog_bytes, sig_bytes):
        raise SignatureError()

    try:
        parsed = json.loads(catalog_bytes)
    except json.JSONDecodeError as exc:
        raise SchemaError(f"catalog.json is not valid JSON: {exc}") from exc

    catalog = validate_catalog_schema(parsed)

    for artifact in catalog["artifacts"]:
        artifact_url = f"{base_url}/{artifact['path']}"
        expected_size = artifact["size"]
        actual_sha256, actual_size = fetch_and_hash_streaming(artifact_url, expected_size, max_attempts=max_attempts)

        if actual_size != expected_size:
            raise SizeMismatch(artifact["path"], expected_size, actual_size)

        expected_sha256 = artifact["sha256"].lower()
        if actual_sha256.lower() != expected_sha256:
            raise Sha256Mismatch(artifact["path"], expected_sha256, actual_sha256)

        print(f"[verify] {artifact['path']}: sha256 ok, size ok ({actual_size} bytes)", flush=True)

    return catalog["catalog_version"], len(catalog["artifacts"])


def is_valid_pubkey_hex(value: str) -> bool:
    value = value.strip()
    if len(value) != 64:
        return False
    try:
        bytes.fromhex(value)
        return True
    except ValueError:
        return False


# ---------------------------------------------------------------------
# --self-test: local stub HTTP servers + a throwaway in-memory key, driven
# via actual subprocesses of this script — see self_test()'s docstring.
# ---------------------------------------------------------------------


class _StubHTTPHandler(http.server.BaseHTTPRequestHandler):
    """Serves fixed bytes from a per-instance `routes` dict; anything not
    in `routes` 404s. A path in `redirects` is served as a 302 with that
    `Location` instead (checked before `routes` — used by self_test()'s
    artifact-redirect scenario; the redirect target is never actually
    fetched by a correct client, so it doesn't need to resolve to anything
    real). Class attributes `routes`/`redirects` are overridden per server
    via a dynamically-built subclass in _start_stub_server — never mutated
    in place, so concurrent scenarios (sequential in this script, but kept
    safe regardless) never share state."""

    routes: dict[str, bytes] = {}
    redirects: dict[str, str] = {}

    def do_GET(self) -> None:  # noqa: N802 - stdlib-mandated method name
        if self.path in self.redirects:
            self.send_response(302)
            self.send_header("Location", self.redirects[self.path])
            self.send_header("Content-Length", "0")
            self.end_headers()
            return
        body = self.routes.get(self.path)
        if body is None:
            self.send_response(404)
            self.send_header("Content-Length", "0")
            self.end_headers()
            return
        self.send_response(200)
        self.send_header("Content-Type", "application/octet-stream")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, format: str, *args) -> None:  # noqa: A002 - silence default stderr access logging
        pass


def _start_stub_server(
    routes: dict[str, bytes], redirects: dict[str, str] | None = None
) -> tuple[str, http.server.ThreadingHTTPServer, threading.Thread]:
    handler_cls = type("_BoundStubHandler", (_StubHTTPHandler,), {"routes": dict(routes), "redirects": dict(redirects or {})})
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler_cls)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    base_url = f"http://127.0.0.1:{server.server_address[1]}"
    return base_url, server, thread


def _stop_stub_server(server: http.server.ThreadingHTTPServer, thread: threading.Thread) -> None:
    server.shutdown()
    server.server_close()
    thread.join(timeout=5)


SELF_TEST_SUBPROCESS_TIMEOUT = 60  # generous ceiling for one CLI run against a local stub server.


def _run_cli(base_url: str, pubkey_hex: str) -> tuple[subprocess.CompletedProcess, float]:
    """Actually spawns `python3 verify_published.py --base-url ... --pubkey
    ...` as a real subprocess — this is the point of a "packet-capture"
    self-check: prove the SCRIPT AS RUN (argument parsing, urllib, retry
    logic, exit code) behaves correctly, not a mock of its internals.
    Returns (completed_process, wall_clock_seconds) — the elapsed time lets
    self_test() additionally assert that a verification failure never
    triggers the retry backoff (which would show up as extra multi-second
    delay), on top of asserting the exit code and FAIL-line text."""
    command = [sys.executable, str(Path(__file__).resolve()), "--base-url", base_url, "--pubkey", pubkey_hex]
    started = time.monotonic()
    completed = subprocess.run(command, capture_output=True, text=True, timeout=SELF_TEST_SUBPROCESS_TIMEOUT)
    elapsed = time.monotonic() - started
    return completed, elapsed


def self_test() -> bool:
    """Exercises verify_published.py end to end against local-only stub
    HTTP servers (stdlib http.server, in daemon threads — never a real
    network call) and a throwaway ed25519 keypair generated in memory for
    this run only (never written anywhere, never CURATOR_KEY_FILE/.env,
    never the production key). Every scenario below actually SPAWNS this
    script as a subprocess (see _run_cli) and asserts its real exit code
    plus the exact FAIL-line content — not just "it errored" — per the
    task brief. Also asserts every FAIL scenario returns FAST (no retry
    backoff fired), since a verification failure must never be retried.

    Scenarios (each is its own isolated stub server + subprocess run):
      (a) happy path: two small fake artifacts, correctly signed catalog
          -> exit 0, "OK: catalog v1, 2 artifacts verified".
      (b) one byte of the SERVED catalog.json flipped from what was
          signed (the .sig itself is untouched) -> signature FAIL.
      (c) one byte of a served artifact flipped (same length as the
          catalog's recorded size) -> sha256-mismatch FAIL.
      (d) an artifact served truncated (shorter than the catalog's
          recorded size) -> size-mismatch FAIL.
      (e) one artifact's URL 404s -> HTTP FAIL naming the status + URL.
      (f) catalog.json.sig served as 65 bytes (one extra byte) -> FAIL
          naming the wrong length.
      (g) two schema violations, each independently signed+served:
          (g1) an artifact missing a required field ("license").
          (g2) an artifact with the wrong base_model.
      (h) an artifact's URL 302-redirects -> redirect FAIL naming the
          status and that the app refuses redirects (see RedirectError) —
          this is the exact "gate passes, app fails" failure mode this
          script exists to catch, since download.rs::streaming_agent()
          pins .redirects(0) and hard-fails on any 3xx there.

    Returns True iff every check across every scenario passes.
    """
    ok = True

    def check(name: str, cond: bool, detail: str = "") -> None:
        nonlocal ok
        status = "PASS" if cond else "FAIL"
        suffix = f" -- {detail!r}" if (detail and not cond) else ""
        print(f"[self-test] {status}: {name}{suffix}", flush=True)
        if not cond:
            ok = False

    # Throwaway keypair, in memory only, generated fresh for this run.
    key = Ed25519PrivateKey.generate()
    seed_hex = key.private_bytes(Encoding.Raw, PrivateFormat.Raw, NoEncryption()).hex()
    pubkey_hex = key.public_key().public_bytes(Encoding.Raw, PublicFormat.Raw).hex()

    base_artifact = (b"FAKE-BASE-GGUF-BYTES-" * 500)[:8192]
    adapter_artifact = (b"FAKE-ADAPTER-GGUF-BYTES-" * 250)[:4096]
    base_sha256 = hashlib.sha256(base_artifact).hexdigest()
    adapter_sha256 = hashlib.sha256(adapter_artifact).hexdigest()
    base_path = "models/Qwen3-4B/v1/fake-base.gguf"
    adapter_path = "adapters/behavioral/v1/Qwen3-4B/fake-adapter.gguf"

    FAST_ENOUGH_SECONDS = 5.0  # a local-only fetch with zero retries never approaches this.

    def make_catalog(*, base_model_override: str | None = None, drop_field: str | None = None, version: int = 1) -> dict:
        base_entry = {
            "path": base_path,
            "sha256": base_sha256,
            "size": len(base_artifact),
            "kind": "base",
            "base_model": "Qwen3-4B",
            "version": "v1",
            "license": "test-license",
        }
        adapter_entry = {
            "path": adapter_path,
            "sha256": adapter_sha256,
            "size": len(adapter_artifact),
            "kind": "adapter",
            "base_model": "Qwen3-4B",
            "version": "v1",
            "license": "test-license",
        }
        if base_model_override is not None:
            base_entry["base_model"] = base_model_override
        if drop_field is not None:
            del base_entry[drop_field]
        return {"catalog_version": version, "generated_at": "2026-07-21T00:00:00Z", "artifacts": [base_entry, adapter_entry]}

    def sign(catalog_dict: dict) -> tuple[bytes, bytes]:
        catalog_bytes = json.dumps(catalog_dict, separators=(",", ":")).encode()
        return catalog_bytes, sign_bytes(seed_hex, catalog_bytes)

    good_catalog_bytes, good_sig = sign(make_catalog())
    good_routes = {
        "/catalog.json": good_catalog_bytes,
        "/catalog.json.sig": good_sig,
        "/" + base_path: base_artifact,
        "/" + adapter_path: adapter_artifact,
    }

    def run_scenario(
        label: str,
        routes: dict[str, bytes],
        *,
        expect_ok: bool,
        expect_substrings: tuple[str, ...],
        redirects: dict[str, str] | None = None,
    ) -> None:
        base_url, server, thread = _start_stub_server(routes, redirects)
        try:
            proc, elapsed = _run_cli(base_url, pubkey_hex)
        finally:
            _stop_stub_server(server, thread)
        combined_output = proc.stdout + proc.stderr
        if expect_ok:
            check(f"{label}: exit 0", proc.returncode == 0, combined_output)
        else:
            check(f"{label}: non-zero exit", proc.returncode != 0, combined_output)
            check(f"{label}: no retry backoff fired ({elapsed:.2f}s)", elapsed < FAST_ENOUGH_SECONDS, combined_output)
        for substring in expect_substrings:
            check(f"{label}: output contains {substring!r}", substring in combined_output, combined_output)

    # (a) happy path.
    run_scenario("(a) happy path", good_routes, expect_ok=True, expect_substrings=("OK: catalog v1, 2 artifacts verified",))

    # (b) tampered catalog byte -> signature FAIL. Sig stays the ORIGINAL
    # good_sig (unchanged) -- it no longer matches the tampered bytes.
    tampered_catalog = bytearray(good_catalog_bytes)
    tampered_catalog[0] ^= 0x01
    routes_b = dict(good_routes)
    routes_b["/catalog.json"] = bytes(tampered_catalog)
    run_scenario(
        "(b) tampered catalog byte",
        routes_b,
        expect_ok=False,
        expect_substrings=("FAIL:", "does NOT verify"),
    )

    # (c) tampered artifact byte (same length) -> sha256 FAIL.
    tampered_artifact = bytearray(base_artifact)
    tampered_artifact[0] ^= 0x01
    routes_c = dict(good_routes)
    routes_c["/" + base_path] = bytes(tampered_artifact)
    run_scenario(
        "(c) tampered artifact byte",
        routes_c,
        expect_ok=False,
        expect_substrings=("FAIL:", "sha256 mismatch"),
    )

    # (d) truncated artifact -> size FAIL.
    routes_d = dict(good_routes)
    routes_d["/" + base_path] = base_artifact[:-16]
    run_scenario(
        "(d) truncated artifact",
        routes_d,
        expect_ok=False,
        expect_substrings=("FAIL:", "size mismatch"),
    )

    # (e) 404 on an artifact -> HTTP FAIL naming status + URL.
    routes_e = {k: v for k, v in good_routes.items() if k != "/" + base_path}
    run_scenario(
        "(e) 404 on an artifact",
        routes_e,
        expect_ok=False,
        expect_substrings=("FAIL:", "HTTP 404", base_path),
    )

    # (f) 65-byte sig -> FAIL naming the wrong length.
    routes_f = dict(good_routes)
    routes_f["/catalog.json.sig"] = good_sig + b"\x00"
    run_scenario(
        "(f) 65-byte sig",
        routes_f,
        expect_ok=False,
        expect_substrings=("FAIL:", "is 65 bytes", "expected exactly 64"),
    )

    # (g1) schema violation: missing required field.
    missing_field_bytes, missing_field_sig = sign(make_catalog(drop_field="license"))
    routes_g1 = {
        "/catalog.json": missing_field_bytes,
        "/catalog.json.sig": missing_field_sig,
        "/" + base_path: base_artifact,
        "/" + adapter_path: adapter_artifact,
    }
    run_scenario(
        "(g1) schema violation: missing field",
        routes_g1,
        expect_ok=False,
        expect_substrings=("FAIL:", "missing field", "license"),
    )

    # (g2) schema violation: an UNKNOWN base_model (not one of the 3 tiers).
    wrong_model_bytes, wrong_model_sig = sign(make_catalog(base_model_override="Not-A-Real-Tier"))
    routes_g2 = {
        "/catalog.json": wrong_model_bytes,
        "/catalog.json.sig": wrong_model_sig,
        "/" + base_path: base_artifact,
        "/" + adapter_path: adapter_artifact,
    }
    run_scenario(
        "(g2) schema violation: wrong base_model",
        routes_g2,
        expect_ok=False,
        expect_substrings=("FAIL:", "base_model", "Not-A-Real-Tier"),
    )

    # (h) artifact URL 302-redirects -> RedirectError FAIL, never followed.
    # The redirect target is a syntactically valid but unfetched path (see
    # _StubHTTPHandler's docstring) -- a correct client must never GET it.
    routes_h = {k: v for k, v in good_routes.items() if k != "/" + base_path}
    run_scenario(
        "(h) artifact redirect",
        routes_h,
        expect_ok=False,
        expect_substrings=("FAIL:", "artifact URL redirected", "HTTP 302", "the app refuses redirects"),
        redirects={"/" + base_path: "/" + adapter_path},
    )

    return ok


# ---------------------------------------------------------------------
# CLI.
# ---------------------------------------------------------------------


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=(
            "Consumer self-check: verify the PUBLISHED catalog.json + every artifact from the public path "
            "only, exactly as the app itself would -- no credentials, no .env, stdlib urllib only."
        ),
        epilog=(
            "Verify the live production bucket (post-handoff, once the production curator public key is\n"
            "known -- must be green before the app wrapper is pointed at cleophis-dist):\n"
            "  ./.venv/bin/python3 verify_published.py --pubkey <64-hex production pubkey>\n\n"
            "Self-test (no network to any real bucket, no credentials, runnable pre-handoff):\n"
            "  ./.venv/bin/python3 verify_published.py --self-test"
        ),
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument("--base-url", default=DEFAULT_BASE_URL, help=f"public artifact base URL (default: {DEFAULT_BASE_URL})")
    parser.add_argument(
        "--pubkey",
        default=None,
        metavar="HEX",
        help="64-hex-char ed25519 curator PUBLIC key (required unless --self-test; no default, no compiled-in key)",
    )
    parser.add_argument(
        "--self-test",
        action="store_true",
        help="run the local-stub-server self-test and exit (no network to any real bucket, no --pubkey/--base-url needed)",
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)

    if args.self_test:
        return 0 if self_test() else 1

    if not args.pubkey:
        print(
            "error: --pubkey is required (unless --self-test) -- a 64-hex-char ed25519 public key; "
            "there is no default or compiled-in key",
            file=sys.stderr,
        )
        return 1
    if not is_valid_pubkey_hex(args.pubkey):
        print(
            f"error: --pubkey must be exactly 64 hex characters (a 32-byte ed25519 public key); "
            f"got {len(args.pubkey.strip())} chars",
            file=sys.stderr,
        )
        return 1

    base_url = args.base_url.rstrip("/")

    try:
        catalog_version, artifact_count = run_verify(base_url, args.pubkey)
    except VerifyFailure as exc:
        print(f"FAIL: {exc}", file=sys.stderr)
        return 1
    except Exception as exc:  # noqa: BLE001 - never let an unexpected exception surface as a raw traceback
        print(f"FAIL: unexpected error verifying published catalog: {type(exc).__name__}: {exc}", file=sys.stderr)
        return 1

    print(f"OK: catalog v{catalog_version}, {artifact_count} artifacts verified", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
