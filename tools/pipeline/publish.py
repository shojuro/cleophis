#!/usr/bin/env python3
"""publish.py — Task A5: upload artifacts + archive-then-replace catalog.

Publishes the already-built, already-signed catalog (`build_catalog.py` +
`sign_catalog.py`, Task A4) to the public `cleophis-dist` bucket: every
artifact `catalog.json["artifacts"][*]` describes, then — last, and only
if every artifact publish succeeded — `catalog.json`/`catalog.json.sig`
themselves, via an archive-then-replace swap that copies whatever catalog
is currently live to the private `cleophis-models` bucket first.

Env-dumb (see README.md "The env-dumb contract"): this script's only
inputs are its CLI flags below and, ONLY when actually publishing (not
--dry-run, not --self-test), `tools/pipeline/.env`'s `B2_ENDPOINT`/
`B2_KEY_ID`/`B2_APP_KEY`. It never reads the calling shell's exported
environment and never assumes a working directory other than its own
`tools/pipeline/` root.

Inputs:
    --catalog        path to the signed catalog.json to publish; its
                      `<catalog>.sig` sibling is read too         (default:
                      work/catalog/catalog.json)
    --out-dir         local artifact root used to resolve each catalog
                      artifact's file — see "Local artifact resolution"
                      below                                        (default:
                      work/out)
    --bucket          destination bucket for artifacts + the live catalog
                                                                    (default:
                      cleophis-dist)
    --archive-bucket  bucket superseded catalogs are archived to   (default:
                      cleophis-models)
    --verify-pubkey   optional pre-flight: verify catalog.json.sig against
                      catalog.json under this 64-hex-char ed25519 public
                      key BEFORE publishing anything (imports `verify_bytes`
                      from sign_catalog.py — no new crypto path)   (default:
                      none — skipped)
    --dry-run         resolve + verify everything from LOCAL STATE ONLY and
                      print the full publish plan; loads no credentials,
                      makes no network call, issues no HEAD/GET/PUT at all
    --self-test       run the fake-client decision-logic self-test and
                      exit; touches no --catalog file, no credentials, no
                      network
    tools/pipeline/.env: B2_ENDPOINT, B2_KEY_ID, B2_APP_KEY (required
                    unless --dry-run/--self-test)

Outputs: none on disk — this script only writes to the two B2 buckets
above. Nothing under `work/` is read except the artifacts/catalog named
above; nothing under `work/` is written.

Local artifact resolution (binding decision for Task A5 — the catalog
carries only a bucket-relative `path`, not a local filesystem path): for
each `catalog.json["artifacts"][i]`, the local file is

    <out-dir>/<artifact.base_model>/Q4_K_M/<basename>   if kind == "base"
    <out-dir>/<artifact.base_model>/adapter/<basename>  if kind == "adapter"

where `<basename>` is the final path component of `artifact.path`. This
mirrors build_base.py's `<out-dir>/<model-name>/<quant>/<name>` and
build_adapter.py's `<out-dir>/<model-name>/adapter/<name>` output layouts
exactly (quant is hardcoded to Q4_K_M — the only quant this pipeline
builds — rather than read from the catalog, which doesn't carry it).

Before ANY upload, every resolved local file is re-hashed (streaming,
never loads the whole file into memory) and its size and sha256 are
compared against the catalog's own recorded values for that artifact;
ANY mismatch — including the file not existing at all — is a hard refusal
before a single network call is made. This script must never upload bytes
the signed catalog doesn't describe: the catalog's signature attests to
specific (path, sha256, size) tuples, and this is the one place those
claims are checked against reality.

Immutability guard, with idempotent resume, for every ARTIFACT path (i.e.
every `catalog.json["artifacts"][i].path`, uploaded to `--bucket`):
    - HEAD the object.
    - Missing -> upload, with `Metadata={"sha256": <hex>}`,
      `Cache-Control: public, max-age=31536000, immutable`, and
      `Content-Type: application/octet-stream`.
    - Exists with `Metadata["sha256"]` == ours -> skip (this is what makes
      a re-run after a partial/interrupted publish resume cleanly instead
      of re-uploading everything).
    - Exists with different/absent `sha256` metadata -> HARD ERROR naming
      the exact path. Artifact paths in `--bucket` are immutable; a
      genuinely new build must publish at a new versioned path (that's
      what `models/<model>/v<n>/...` / `adapters/.../v<n>/...` are for) —
      this script never overwrites one in place, no exceptions.

Catalog replace, archive-first, fail-closed (`catalog.json`/
`catalog.json.sig` are the ONLY objects in `--bucket` this script ever
replaces in place):
    (a) GET the current live `catalog.json` + `.sig` from `--bucket`, if
        they exist.
    (b) If they exist: parse the old catalog's own `catalog_version`
        (refusing — before touching anything — if it's missing, not an
        integer, or outside a sane bound; a corrupted/poisoned live
        catalog must not silently drive an archive path), then copy BOTH
        files, byte-for-byte as fetched, to `--archive-bucket` at
        `archive/catalogs/v<old_catalog_version>/catalog.json` and
        `...catalog.json.sig`. Archive objects are immutable too — the
        exact same HEAD-then-compare-sha256 guard as artifacts above
        applies to them (missing -> archive; matching content already
        there -> skip/resume; mismatched content already there -> hard
        error, never overwritten).
    (c) ONLY once (b) has fully succeeded (or wasn't needed — see below)
        does this script write the NEW `catalog.json` (Content-Type
        application/json) then the NEW `catalog.json.sig` (Content-Type
        application/octet-stream), both with `Cache-Control: public,
        max-age=60`. Any failure in (a)/(b) — a network error that
        exhausts its retries, an unparseable old catalog, an inconsistent
        remote state (one of the pair present but not the other), or an
        archive-immutability violation — raises BEFORE either of these two
        PUTs ever runs; the live catalog is never touched on a failed
        archive.
    First-ever publish (no current `catalog.json` in `--bucket` at all) ->
    archiving is skipped cleanly, straight to (c).

Ordering: every artifact is published FIRST; the catalog swap is always
LAST, and only attempted if every artifact publish above succeeded. This
means a client can never observe a live catalog.json whose artifacts
aren't actually present yet.

The two live-catalog PUTs in step (c) are NOT atomic with each other
(there is no cross-object transaction in the S3 API this bucket exposes).
This is deliberately safe anyway: catalog.json is written before
catalog.json.sig, so during that brief window a client fetching the pair
sees either (OLD catalog.json, OLD .sig) or (NEW catalog.json, OLD .sig)
— and `kpack_core::sign::verify_detached` verifies the .sig against the
EXACT bytes of whatever catalog.json it just fetched. A signature over the
old bytes never verifies against the new bytes (or vice versa), so a
client caught in this window fails signature verification and rejects the
fetch — it fails CLOSED and is expected to retry, never trusting a
mismatched pairing. The same reasoning holds regardless of which of the
two objects is written first; step (c)'s catalog-then-sig order is simply
what's specified for this pipeline.

Retries: every S3 operation (HEAD/GET/PUT/upload) gets the house 3-attempt
bounded retry (mirrors build_adapter.py's B2 download loop) — except a
definitive "object does not exist" response, which is not a failure and
is never retried. Credentials are never logged; every log line names only
the bucket/key (or a shlex-quoted local path) being acted on.

Testability: all S3 I/O is behind the small `head`/`get_bytes`/
`upload_file`/`put_bytes` interface `S3Client` implements. Every actual
decision — `publish_artifact`, `_put_immutable`, `swap_catalog` — is
written against that interface, not against boto3 directly, so
`--self-test` can exercise all of it against `_FakeS3Client` (a hand-
rolled, in-memory, boto3-free stand-in) with zero network. See
`self_test()`'s cases: immutability refusal on a mismatched existing
artifact, skip-on-matching-metadata (idempotent resume), archive-before-
replace call ORDER, an archive failure aborting the catalog swap before
the live catalog is touched, first-publish skipping the archive step
cleanly, and local-hash-mismatch refusal.

`--dry-run` end-to-end (no creds, no network — see the module CLI help):
    ./.venv/bin/python3 publish.py --dry-run

`--self-test` (no creds, no network, no --catalog needed):
    ./.venv/bin/python3 publish.py --self-test

Live publish, once B2 creds exist (post-handoff):
    ./.venv/bin/python3 publish.py
"""

from __future__ import annotations

import argparse
import hashlib
import json
import shlex
import sys
import tempfile
import time
from pathlib import Path

from sign_catalog import verify_bytes  # Task A5 binding decision — reuse this one crypto path, no new one.

PIPELINE_ROOT = Path(__file__).resolve().parent
ENV_FILE = PIPELINE_ROOT / ".env"

DEFAULT_CATALOG = PIPELINE_ROOT / "work" / "catalog" / "catalog.json"
DEFAULT_OUT_DIR = PIPELINE_ROOT / "work" / "out"
DEFAULT_BUCKET = "cleophis-dist"
DEFAULT_ARCHIVE_BUCKET = "cleophis-models"
DEFAULT_MAX_ATTEMPTS = 3

HASH_CHUNK_SIZE = 1024 * 1024  # 1 MiB — stream every hash, never slurp a file.

SIG_BYTES_LEN = 64  # matches sign_catalog.py's SIG_BYTES_LEN / kpack_core::sign's read_sig_capped.
SIG_READ_CAP = 4096  # generous INFRA-level cap on a remote-fetched .sig; the exact-64 check is the business rule.
MAX_CATALOG_BYTES = 1024 * 1024  # 1 MiB — mirrors sign_catalog.py's own cap on catalog.json.

# Mirrors build_catalog.py's check_version_in_bounds ceiling: a catalog_version
# read off a REMOTE (in this script's case: the currently-live, but still
# externally-sourced) catalog.json must be sanity-bounded before it's used to
# build an archive path — see swap_catalog()'s docstring.
MAX_SANE_CATALOG_VERSION = 1_000_000

ARTIFACT_CONTENT_TYPE = "application/octet-stream"
ARTIFACT_CACHE_CONTROL = "public, max-age=31536000, immutable"
CATALOG_CONTENT_TYPE = "application/json"
SIG_CONTENT_TYPE = "application/octet-stream"
CATALOG_CACHE_CONTROL = "public, max-age=60"
ARCHIVE_CACHE_CONTROL = "public, max-age=31536000, immutable"  # archived catalogs are immutable too.

CATALOG_KEY = "catalog.json"
SIG_KEY = "catalog.json.sig"

# Local output subdirectory per catalog artifact "kind" — see the module
# docstring's "Local artifact resolution" section. Duplicated here (not
# imported from build_base.py/build_adapter.py/build_catalog.py) so this
# script stays a standalone, env-dumb unit, matching the house pattern those
# scripts already set for their own duplicated cross-track constants.
LOCAL_ARTIFACT_SUBDIR_BY_KIND = {"base": "Q4_K_M", "adapter": "adapter"}

REQUIRED_ARTIFACT_FIELDS = ("path", "sha256", "size", "kind", "base_model")


class PublishError(RuntimeError):
    """Base class for every clean refusal in this script. Always caught at
    main()'s top level and reported as a one-line error, never a raw
    traceback — and never one that includes credentials."""


class LocalHashMismatch(PublishError):
    """A local artifact file is missing, or its size/sha256 doesn't match
    what the signed catalog claims for it. publish.py must never upload
    bytes the signed catalog doesn't describe."""


class ImmutabilityViolation(PublishError):
    """A remote object (artifact or archived catalog) already exists with
    different content than what this run would write. Immutable paths are
    never overwritten — this is always a hard abort, never a retry."""


class RemoteStateError(PublishError):
    """The remote bucket is in a state this script doesn't know how to
    safely proceed from (e.g. only one of catalog.json/.sig present, an
    unparseable/out-of-bounds live catalog_version, an oversized GET)."""


class S3OpError(PublishError):
    """A real S3 operation failed after exhausting its retry budget."""


# ---------------------------------------------------------------------
# .env (B2 credentials) — same pattern as build_adapter.py/build_base.py.
# ---------------------------------------------------------------------


def load_env_file(env_file: Path) -> dict[str, str]:
    """Parse tools/pipeline/.env into a dict; {} if the file doesn't exist.
    Only this one file is consulted — see the env-dumb contract."""
    values: dict[str, str] = {}
    if not env_file.is_file():
        return values
    for raw_line in env_file.read_text().splitlines():
        line = raw_line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, _, value = line.partition("=")
        values[key.strip()] = value.strip().strip('"').strip("'")
    return values


def load_b2_credentials(env_file: Path) -> tuple[str, str, str] | None:
    """Read B2_ENDPOINT/B2_KEY_ID/B2_APP_KEY from .env; None if any are
    missing/unset."""
    values = load_env_file(env_file)
    endpoint = values.get("B2_ENDPOINT") or None
    key_id = values.get("B2_KEY_ID") or None
    app_key = values.get("B2_APP_KEY") or None
    if not (endpoint and key_id and app_key):
        return None
    return endpoint, key_id, app_key


# ---------------------------------------------------------------------
# Local catalog/.sig loading (stat-before-read caps — mirrors
# sign_catalog.py's own MAX_CATALOG_BYTES check and the Rust
# read_sig_capped pattern for the .sig file).
# ---------------------------------------------------------------------


def load_catalog(catalog_path: Path) -> tuple[dict, bytes]:
    """Reads + parses catalog.json, returning (parsed dict, exact raw
    bytes read) — the exact bytes are what get re-uploaded verbatim and
    are what --verify-pubkey checks the signature against, never a
    re-serialization of the parsed object."""
    if not catalog_path.is_file():
        raise PublishError(f"{catalog_path} not found — run build_catalog.py + sign_catalog.py first")
    size = catalog_path.stat().st_size
    if size > MAX_CATALOG_BYTES:
        raise PublishError(
            f"{catalog_path} is {size} bytes, exceeding the {MAX_CATALOG_BYTES}-byte sanity cap "
            "(matches sign_catalog.py's own cap and the app's own fetch cap) — refusing to read"
        )
    data = catalog_path.read_bytes()
    try:
        parsed = json.loads(data)
    except json.JSONDecodeError as exc:
        raise PublishError(f"{catalog_path} is not valid JSON: {exc}") from exc
    if not isinstance(parsed, dict):
        raise PublishError(f"{catalog_path} does not contain a JSON object")
    if not isinstance(parsed.get("artifacts"), list) or not parsed["artifacts"]:
        raise PublishError(f"{catalog_path} has no non-empty 'artifacts' list")
    version = parsed.get("catalog_version")
    if not isinstance(version, int) or isinstance(version, bool):
        raise PublishError(f"{catalog_path} has no integer 'catalog_version' field")
    return parsed, data


def read_sig_exact(sig_path: Path) -> bytes:
    """Reads `<catalog>.sig`, refusing (without reading its content) if it
    is not EXACTLY 64 bytes — a raw detached ed25519 signature always is.
    Mirrors kpack_core::sign's read_sig_capped: the size check happens via
    stat() before any read, so an oversized file is never buffered just to
    be rejected."""
    if not sig_path.is_file():
        raise PublishError(f"{sig_path} not found — run sign_catalog.py first")
    size = sig_path.stat().st_size
    if size != SIG_BYTES_LEN:
        raise PublishError(
            f"{sig_path} is {size} bytes, expected exactly {SIG_BYTES_LEN} (a raw detached ed25519 "
            "signature) — refusing to treat this as a valid signature"
        )
    return sig_path.read_bytes()


def validate_artifact_entry(artifact: object, index: int) -> None:
    """Light structural validation of one catalog artifact entry — this
    script trusts build_catalog.py/sign_catalog.py for the FULL wire-format
    contract, but still refuses cleanly rather than crashing with a raw
    KeyError/TypeError on a hand-edited or corrupted catalog.json."""
    if not isinstance(artifact, dict):
        raise PublishError(f"catalog artifacts[{index}] is not a JSON object")
    missing = [f for f in REQUIRED_ARTIFACT_FIELDS if f not in artifact]
    if missing:
        raise PublishError(f"catalog artifacts[{index}] is missing field(s) {missing}")
    kind = artifact["kind"]
    if kind not in LOCAL_ARTIFACT_SUBDIR_BY_KIND:
        raise PublishError(
            f"catalog artifacts[{index}] has kind={kind!r}, expected one of {sorted(LOCAL_ARTIFACT_SUBDIR_BY_KIND)}"
        )
    sha256 = artifact["sha256"]
    if not (isinstance(sha256, str) and len(sha256) == 64 and all(c in "0123456789abcdef" for c in sha256.lower())):
        raise PublishError(f"catalog artifacts[{index}] sha256={sha256!r} is not a 64-hex-char digest")
    size = artifact["size"]
    if not isinstance(size, int) or isinstance(size, bool) or size <= 0:
        raise PublishError(f"catalog artifacts[{index}] size={size!r} is not a positive integer")
    path = artifact["path"]
    if not isinstance(path, str) or not path:
        raise PublishError(f"catalog artifacts[{index}] path={path!r} is not a non-empty string")
    base_model = artifact["base_model"]
    if not isinstance(base_model, str) or not base_model:
        raise PublishError(f"catalog artifacts[{index}] base_model={base_model!r} is not a non-empty string")


# ---------------------------------------------------------------------
# Local artifact resolution + pre-upload hash verification.
# ---------------------------------------------------------------------


def local_artifact_path(out_dir: Path, artifact: dict) -> Path:
    """See the module docstring's "Local artifact resolution" section."""
    subdir = LOCAL_ARTIFACT_SUBDIR_BY_KIND[artifact["kind"]]
    basename = Path(artifact["path"]).name
    return out_dir / artifact["base_model"] / subdir / basename


def sha256_file(path: Path) -> str:
    """Streaming sha256 — never loads the whole file into RAM."""
    digest = hashlib.sha256()
    with path.open("rb") as f:
        while True:
            chunk = f.read(HASH_CHUNK_SIZE)
            if not chunk:
                break
            digest.update(chunk)
    return digest.hexdigest()


def verify_local_artifact(local_path: Path, artifact: dict) -> None:
    """Refuses (LocalHashMismatch) if `local_path` doesn't exist, its size
    doesn't match `artifact["size"]`, or its streaming sha256 doesn't match
    `artifact["sha256"]`. The size check runs first and fails fast — on a
    mismatch there's no point streaming a multi-GB hash just to prove what
    the size already disproved."""
    if not local_path.is_file():
        raise LocalHashMismatch(
            f"{local_path} not found locally for catalog artifact {artifact['path']!r} — build it first "
            "(see build_base.py/build_adapter.py)"
        )
    expected_size = artifact["size"]
    actual_size = local_path.stat().st_size
    if actual_size != expected_size:
        raise LocalHashMismatch(
            f"{local_path} is {actual_size} bytes, catalog says {expected_size} for {artifact['path']!r} — "
            "refusing to publish a file that doesn't match the signed catalog"
        )
    expected_sha256 = artifact["sha256"].lower()
    actual_sha256 = sha256_file(local_path)
    if actual_sha256 != expected_sha256:
        raise LocalHashMismatch(
            f"{local_path} sha256={actual_sha256}, catalog says {expected_sha256} for {artifact['path']!r} — "
            "refusing to upload bytes the signed catalog doesn't describe"
        )


# ---------------------------------------------------------------------
# Decision logic — written ONLY against the S3Client interface (head/
# get_bytes/upload_file/put_bytes), so it's exercised identically whether
# the caller passes a real S3Client or the fake one --self-test uses.
# ---------------------------------------------------------------------


def publish_artifact(client, bucket: str, artifact: dict, local_path: Path) -> str:
    """Immutability guard with idempotent resume for one artifact path.
    Returns "uploaded" or "skipped"; raises ImmutabilityViolation if the
    object already exists with different/absent sha256 metadata. Trusts
    `artifact["sha256"]` as ground truth — the caller (main()) already ran
    verify_local_artifact on `local_path` before this is ever called, so
    re-hashing the (possibly multi-GB) file here would be redundant."""
    key = artifact["path"]
    sha256_hex = artifact["sha256"].lower()
    existing = client.head(bucket, key)
    if existing is None:
        client.upload_file(
            bucket,
            key,
            local_path,
            content_type=ARTIFACT_CONTENT_TYPE,
            cache_control=ARTIFACT_CACHE_CONTROL,
            metadata={"sha256": sha256_hex},
        )
        return "uploaded"
    existing_sha256 = existing.get("sha256")
    if existing_sha256 == sha256_hex:
        return "skipped"
    raise ImmutabilityViolation(
        f"s3://{bucket}/{key} already exists with sha256 metadata {existing_sha256!r}, which does not "
        f"match this build's {sha256_hex!r} — artifact paths in {bucket} are immutable, refusing to "
        "overwrite. A genuinely new build must publish at a new versioned path."
    )


def _put_immutable(client, bucket: str, key: str, data: bytes, content_type: str) -> str:
    """Same immutability-with-resume guard as publish_artifact, for an
    in-memory blob — used to archive the superseded catalog.json/.sig.
    Returns "archived" or "skipped"; raises ImmutabilityViolation on a
    content mismatch against what's already there."""
    sha256_hex = hashlib.sha256(data).hexdigest()
    existing = client.head(bucket, key)
    if existing is None:
        client.put_bytes(bucket, key, data, content_type=content_type, cache_control=ARCHIVE_CACHE_CONTROL, metadata={"sha256": sha256_hex})
        return "archived"
    existing_sha256 = existing.get("sha256")
    if existing_sha256 == sha256_hex:
        return "skipped"
    raise ImmutabilityViolation(
        f"s3://{bucket}/{key} (archive) already exists with sha256 metadata {existing_sha256!r}, which "
        f"does not match {sha256_hex!r} — archive objects are immutable too, refusing to overwrite."
    )


def swap_catalog(client, dest_bucket: str, archive_bucket: str, new_catalog_bytes: bytes, new_sig_bytes: bytes) -> str:
    """Archive-then-replace, fail-closed. If a live catalog.json currently
    exists at `dest_bucket`, it (and its .sig) are archived to
    `archive_bucket` at `archive/catalogs/v<old_catalog_version>/` FIRST;
    only if that fully succeeds are the new catalog.json/.sig ever written
    to `dest_bucket`. Any failure along the way — an inconsistent remote
    state (exactly one of the pair present), an unparseable or
    out-of-bounds old catalog_version, an archive-immutability violation,
    or a transient S3 failure that exhausts its retries — raises before
    `dest_bucket`'s live catalog.json/.sig are ever touched. See the
    module docstring's "Ordering" section for why the two live PUTs below
    not being atomic with each other is still safe.

    Returns "archived-then-replaced" or "first-publish".
    """
    current_catalog = client.get_bytes(dest_bucket, CATALOG_KEY, MAX_CATALOG_BYTES)
    current_sig = client.get_bytes(dest_bucket, SIG_KEY, SIG_READ_CAP)

    if current_catalog is None and current_sig is None:
        archived = False
        print(f"[archive] no current catalog.json at s3://{dest_bucket}/{CATALOG_KEY} — first-ever publish, skipping archive", flush=True)
    elif current_catalog is None or current_sig is None:
        raise RemoteStateError(
            f"s3://{dest_bucket} has exactly one of {CATALOG_KEY}/{SIG_KEY} present, not both — inconsistent "
            "remote state, refusing to archive or replace. Investigate manually before retrying."
        )
    else:
        if len(current_sig) != SIG_BYTES_LEN:
            raise RemoteStateError(
                f"current live s3://{dest_bucket}/{SIG_KEY} is {len(current_sig)} bytes, expected exactly "
                f"{SIG_BYTES_LEN} — refusing to archive/replace"
            )
        try:
            old_catalog_obj = json.loads(current_catalog)
        except json.JSONDecodeError as exc:
            raise RemoteStateError(
                f"current live s3://{dest_bucket}/{CATALOG_KEY} is not valid JSON, refusing to archive/replace: {exc}"
            ) from exc
        if not isinstance(old_catalog_obj, dict):
            raise RemoteStateError(f"current live s3://{dest_bucket}/{CATALOG_KEY} is not a JSON object, refusing to archive/replace")
        old_version = old_catalog_obj.get("catalog_version")
        if not isinstance(old_version, int) or isinstance(old_version, bool):
            raise RemoteStateError(
                f"current live s3://{dest_bucket}/{CATALOG_KEY} has no integer catalog_version, refusing to archive/replace"
            )
        if not (0 <= old_version < MAX_SANE_CATALOG_VERSION):
            # Same defensive rationale as build_catalog.py's check_version_in_bounds:
            # a poisoned/corrupted live catalog must not silently drive an archive
            # path built from an attacker-influenceable integer.
            raise RemoteStateError(
                f"current live s3://{dest_bucket}/{CATALOG_KEY} catalog_version={old_version} is outside the "
                f"sane bound [0, {MAX_SANE_CATALOG_VERSION}) — refusing to trust it for an archive path"
            )

        archive_prefix = f"archive/catalogs/v{old_version}/"
        print(f"[archive] archiving current catalog_version={old_version} to s3://{archive_bucket}/{archive_prefix}", flush=True)
        result_catalog = _put_immutable(client, archive_bucket, archive_prefix + CATALOG_KEY, current_catalog, CATALOG_CONTENT_TYPE)
        print(f"[archive] {result_catalog}: s3://{archive_bucket}/{archive_prefix}{CATALOG_KEY}", flush=True)
        result_sig = _put_immutable(client, archive_bucket, archive_prefix + SIG_KEY, current_sig, SIG_CONTENT_TYPE)
        print(f"[archive] {result_sig}: s3://{archive_bucket}/{archive_prefix}{SIG_KEY}", flush=True)
        archived = True

    # Only reached once archiving succeeded (or wasn't needed).
    client.put_bytes(dest_bucket, CATALOG_KEY, new_catalog_bytes, content_type=CATALOG_CONTENT_TYPE, cache_control=CATALOG_CACHE_CONTROL, metadata=None)
    print(f"[publish] wrote s3://{dest_bucket}/{CATALOG_KEY}", flush=True)
    client.put_bytes(dest_bucket, SIG_KEY, new_sig_bytes, content_type=SIG_CONTENT_TYPE, cache_control=CATALOG_CACHE_CONTROL, metadata=None)
    print(f"[publish] wrote s3://{dest_bucket}/{SIG_KEY}", flush=True)

    return "archived-then-replaced" if archived else "first-publish"


# ---------------------------------------------------------------------
# S3Client — the only class that ever touches boto3/network. boto3 is
# imported lazily inside __init__ so --dry-run and --self-test never
# require it to even be importable.
# ---------------------------------------------------------------------

_NOT_FOUND = object()  # internal sentinel distinguishing "confirmed absent" from a transient failure.
NOT_FOUND_CODES = {"404", "NoSuchKey", "NotFound"}


class S3Client:
    """Thin wrapper around a boto3 S3-compatible client (B2's S3-compatible
    API). Exposes exactly the interface the decision functions above need:
    head/get_bytes/upload_file/put_bytes — nothing else, so `_FakeS3Client`
    below can stand in for it in --self-test with zero network and zero
    boto3 dependency."""

    def __init__(self, endpoint: str, key_id: str, app_key: str, max_attempts: int = DEFAULT_MAX_ATTEMPTS):
        import boto3
        from botocore.exceptions import ClientError

        self._ClientError = ClientError
        self._client = boto3.client(
            "s3",
            endpoint_url=endpoint,
            aws_access_key_id=key_id,
            aws_secret_access_key=app_key,
        )
        self.max_attempts = max_attempts

    def _retry(self, description: str, fn):
        """House 3-attempt bounded retry (mirrors build_adapter.py's B2
        download loop): any exception is retried with a 5*attempt-second
        backoff, up to max_attempts, then wrapped in S3OpError. Never
        retries a definitive "not found" — see the `_NOT_FOUND` sentinel
        handling in head()/get_bytes() below, which resolves that case
        inside `fn()` without raising at all."""
        last_exc: Exception | None = None
        for attempt in range(1, self.max_attempts + 1):
            try:
                return fn()
            except Exception as exc:  # noqa: BLE001 - deliberately broad: retry any transient S3 failure
                last_exc = exc
                print(f"[s3] {description} attempt {attempt}/{self.max_attempts} failed: {exc!r}", flush=True)
                if attempt < self.max_attempts:
                    delay = 5 * attempt
                    print(f"[s3] retrying in {delay}s...", flush=True)
                    time.sleep(delay)
        raise S3OpError(f"{description} failed after {self.max_attempts} attempts") from last_exc

    def head(self, bucket: str, key: str) -> dict | None:
        def op():
            try:
                resp = self._client.head_object(Bucket=bucket, Key=key)
            except self._ClientError as exc:
                code = exc.response.get("Error", {}).get("Code", "")
                if code in NOT_FOUND_CODES:
                    return _NOT_FOUND
                raise
            return dict(resp.get("Metadata", {}))

        result = self._retry(f"HEAD s3://{bucket}/{key}", op)
        return None if result is _NOT_FOUND else result

    def get_bytes(self, bucket: str, key: str, max_bytes: int) -> bytes | None:
        """None if the object doesn't exist. Raises RemoteStateError if the
        object exists but is larger than `max_bytes` — read via a capped
        `.read(max_bytes + 1)` so an oversized/corrupted remote object is
        never buffered in full just to be rejected."""

        def op():
            try:
                resp = self._client.get_object(Bucket=bucket, Key=key)
            except self._ClientError as exc:
                code = exc.response.get("Error", {}).get("Code", "")
                if code in NOT_FOUND_CODES:
                    return _NOT_FOUND
                raise
            return resp["Body"].read(max_bytes + 1)

        result = self._retry(f"GET s3://{bucket}/{key}", op)
        if result is _NOT_FOUND:
            return None
        if len(result) > max_bytes:
            raise RemoteStateError(
                f"GET s3://{bucket}/{key} returned more than the {max_bytes}-byte sanity cap — refusing to "
                "buffer an oversized/corrupted remote object"
            )
        return result

    def upload_file(self, bucket: str, key: str, local_path: Path, *, content_type: str, cache_control: str, metadata: dict[str, str]) -> None:
        extra_args = {"ContentType": content_type, "CacheControl": cache_control, "Metadata": metadata}

        def op():
            self._client.upload_file(str(local_path), bucket, key, ExtraArgs=extra_args)

        self._retry(f"upload s3://{bucket}/{key}", op)

    def put_bytes(self, bucket: str, key: str, data: bytes, *, content_type: str, cache_control: str, metadata: dict[str, str] | None) -> None:
        kwargs: dict = {"Bucket": bucket, "Key": key, "Body": data, "ContentType": content_type, "CacheControl": cache_control}
        if metadata:
            kwargs["Metadata"] = metadata

        def op():
            self._client.put_object(**kwargs)

        self._retry(f"PUT s3://{bucket}/{key}", op)


class _FakeS3Client:
    """Hand-rolled, in-memory, boto3-free stand-in for S3Client — used
    ONLY by --self-test to exercise the decision logic above
    (publish_artifact/_put_immutable/swap_catalog) against a controllable
    fake remote. Records every call in `self.calls` as (op, bucket, key)
    tuples, in order, so tests can assert call ORDER (e.g. every
    archive-bucket write happens before the dest-bucket catalog writes).
    `fail_puts` simulates a specific (bucket, key) PUT/upload always
    failing, for the archive-failure-aborts-the-swap test.
    """

    def __init__(self, fail_puts: set[tuple[str, str]] | None = None):
        self.store: dict[tuple[str, str], dict] = {}
        self.calls: list[tuple[str, str, str]] = []
        self._fail_puts = fail_puts or set()

    def _seed(self, bucket: str, key: str, data: bytes, sha256_hex: str | None = None) -> None:
        """Test setup helper — not part of the S3Client interface itself."""
        self.store[(bucket, key)] = {"data": data, "metadata": {"sha256": sha256_hex} if sha256_hex else {}}

    def head(self, bucket: str, key: str) -> dict | None:
        self.calls.append(("head", bucket, key))
        obj = self.store.get((bucket, key))
        return dict(obj["metadata"]) if obj is not None else None

    def get_bytes(self, bucket: str, key: str, max_bytes: int) -> bytes | None:
        self.calls.append(("get", bucket, key))
        obj = self.store.get((bucket, key))
        if obj is None:
            return None
        if len(obj["data"]) > max_bytes:
            raise RemoteStateError(f"GET s3://{bucket}/{key} returned more than the {max_bytes}-byte sanity cap")
        return obj["data"]

    def upload_file(self, bucket: str, key: str, local_path: Path, *, content_type: str, cache_control: str, metadata: dict[str, str]) -> None:
        self.calls.append(("upload_file", bucket, key))
        if (bucket, key) in self._fail_puts:
            raise RuntimeError(f"simulated upload failure for s3://{bucket}/{key}")
        self.store[(bucket, key)] = {"data": Path(local_path).read_bytes(), "metadata": dict(metadata)}

    def put_bytes(self, bucket: str, key: str, data: bytes, *, content_type: str, cache_control: str, metadata: dict[str, str] | None) -> None:
        self.calls.append(("put_bytes", bucket, key))
        if (bucket, key) in self._fail_puts:
            raise RuntimeError(f"simulated PUT failure for s3://{bucket}/{key}")
        self.store[(bucket, key)] = {"data": data, "metadata": dict(metadata) if metadata else {}}


# ---------------------------------------------------------------------
# --self-test: fake-client decision-logic checks (mirrors A4's pattern).
# ---------------------------------------------------------------------


def self_test() -> bool:
    """Local, network-free, boto3-free checks of the decision logic
    against `_FakeS3Client`. Returns True iff every check passes. See the
    module docstring's "Testability" section for what's covered."""
    ok = True

    def check(name: str, cond: bool) -> None:
        nonlocal ok
        print(f"[self-test] {'PASS' if cond else 'FAIL'}: {name}", flush=True)
        if not cond:
            ok = False

    def raises(exc_type, fn) -> bool:
        try:
            fn()
            return False
        except exc_type:
            return True
        except Exception:
            return False

    artifact = {
        "path": "models/Qwen3-4B/v1/Qwen3-4B-Instruct-Q4_K_M.gguf",
        "sha256": "a" * 64,
        "size": 10,
        "kind": "base",
        "base_model": "Qwen3-4B",
    }

    with tempfile.TemporaryDirectory() as tmp:
        local = Path(tmp) / "artifact.gguf"
        local.write_bytes(b"x" * 10)

        # --- publish_artifact: immutability refusal on mismatched existing artifact ---
        client_mismatch = _FakeS3Client()
        client_mismatch._seed(DEFAULT_BUCKET, artifact["path"], b"old-bytes", sha256_hex="b" * 64)
        check(
            "publish_artifact: mismatched existing sha256 metadata raises ImmutabilityViolation",
            raises(ImmutabilityViolation, lambda: publish_artifact(client_mismatch, DEFAULT_BUCKET, artifact, local)),
        )

        # --- publish_artifact: skip on matching metadata (idempotent resume) ---
        client_match = _FakeS3Client()
        client_match._seed(DEFAULT_BUCKET, artifact["path"], b"same-content-marker", sha256_hex="a" * 64)
        result_match = publish_artifact(client_match, DEFAULT_BUCKET, artifact, local)
        check("publish_artifact: matching existing sha256 metadata is skipped, not re-uploaded", result_match == "skipped")
        check("publish_artifact: a skip issues no upload_file call", all(c[0] != "upload_file" for c in client_match.calls))

        # --- publish_artifact: missing object uploads ---
        client_fresh = _FakeS3Client()
        result_fresh = publish_artifact(client_fresh, DEFAULT_BUCKET, artifact, local)
        check("publish_artifact: missing object uploads", result_fresh == "uploaded")
        check(
            "publish_artifact: uploaded object is stored with sha256 metadata",
            client_fresh.store[(DEFAULT_BUCKET, artifact["path"])]["metadata"].get("sha256") == artifact["sha256"],
        )

        # --- local-hash-mismatch refusal ---
        local2 = Path(tmp) / "artifact2.gguf"
        local2.write_bytes(b"actual content")
        wrong_artifact = dict(artifact, sha256="0" * 64, size=len(b"actual content"))
        check(
            "verify_local_artifact: sha256 mismatch raises LocalHashMismatch",
            raises(LocalHashMismatch, lambda: verify_local_artifact(local2, wrong_artifact)),
        )
        right_artifact = dict(artifact, sha256=hashlib.sha256(b"actual content").hexdigest(), size=len(b"actual content"))
        no_raise = True
        try:
            verify_local_artifact(local2, right_artifact)
        except LocalHashMismatch:
            no_raise = False
        check("verify_local_artifact: matching sha256+size does not raise", no_raise)
        size_mismatch_artifact = dict(right_artifact, size=999999)
        check(
            "verify_local_artifact: size mismatch raises LocalHashMismatch (fails fast before hashing)",
            raises(LocalHashMismatch, lambda: verify_local_artifact(local2, size_mismatch_artifact)),
        )
        missing_path = Path(tmp) / "does-not-exist.gguf"
        check(
            "verify_local_artifact: missing local file raises LocalHashMismatch",
            raises(LocalHashMismatch, lambda: verify_local_artifact(missing_path, right_artifact)),
        )

        # --- read_sig_exact: exact-64-byte pre-flight check ---
        sig_path = Path(tmp) / "catalog.json.sig"
        sig_path.write_bytes(b"too-short")
        check("read_sig_exact: a non-64-byte .sig is refused", raises(PublishError, lambda: read_sig_exact(sig_path)))
        sig_path.write_bytes(b"s" * 64)
        check("read_sig_exact: an exactly-64-byte .sig is accepted", read_sig_exact(sig_path) == b"s" * 64)

    new_catalog = json.dumps({"catalog_version": 1, "artifacts": []}).encode()
    new_sig = b"n" * 64

    # --- swap_catalog: first-publish skips archive ---
    client_first = _FakeS3Client()
    result_first = swap_catalog(client_first, DEFAULT_BUCKET, DEFAULT_ARCHIVE_BUCKET, new_catalog, new_sig)
    check("swap_catalog: first-ever publish returns 'first-publish'", result_first == "first-publish")
    check(
        "swap_catalog: first-ever publish never touches the archive bucket",
        all(c[1] != DEFAULT_ARCHIVE_BUCKET for c in client_first.calls),
    )
    check(
        "swap_catalog: first-ever publish writes the new catalog.json + .sig",
        client_first.store[(DEFAULT_BUCKET, CATALOG_KEY)]["data"] == new_catalog
        and client_first.store[(DEFAULT_BUCKET, SIG_KEY)]["data"] == new_sig,
    )

    # --- swap_catalog: archive-before-replace ordering (assert call order) ---
    old_catalog = json.dumps({"catalog_version": 7, "artifacts": []}).encode()
    old_sig = b"o" * 64
    client_swap = _FakeS3Client()
    client_swap._seed(DEFAULT_BUCKET, CATALOG_KEY, old_catalog)
    client_swap._seed(DEFAULT_BUCKET, SIG_KEY, old_sig)
    result_swap = swap_catalog(client_swap, DEFAULT_BUCKET, DEFAULT_ARCHIVE_BUCKET, new_catalog, new_sig)
    archive_prefix = "archive/catalogs/v7/"
    check("swap_catalog: existing live catalog returns 'archived-then-replaced'", result_swap == "archived-then-replaced")
    check(
        "swap_catalog: old catalog.json archived at archive/catalogs/v<old_version>/",
        client_swap.store.get((DEFAULT_ARCHIVE_BUCKET, archive_prefix + CATALOG_KEY), {}).get("data") == old_catalog,
    )
    check(
        "swap_catalog: old catalog.json.sig archived alongside it",
        client_swap.store.get((DEFAULT_ARCHIVE_BUCKET, archive_prefix + SIG_KEY), {}).get("data") == old_sig,
    )
    check(
        "swap_catalog: new catalog.json/.sig replace the live ones",
        client_swap.store[(DEFAULT_BUCKET, CATALOG_KEY)]["data"] == new_catalog
        and client_swap.store[(DEFAULT_BUCKET, SIG_KEY)]["data"] == new_sig,
    )
    archive_write_indices = [i for i, c in enumerate(client_swap.calls) if c[1] == DEFAULT_ARCHIVE_BUCKET and c[0] in ("upload_file", "put_bytes")]
    dest_write_indices = [i for i, c in enumerate(client_swap.calls) if c[1] == DEFAULT_BUCKET and c[0] in ("upload_file", "put_bytes")]
    check(
        "swap_catalog: EVERY archive-bucket write happens before EVERY dest-bucket catalog write (call-order assertion)",
        bool(archive_write_indices) and bool(dest_write_indices) and max(archive_write_indices) < min(dest_write_indices),
    )

    # --- swap_catalog: archive-failure aborts the catalog swap ---
    client_fail = _FakeS3Client(fail_puts={(DEFAULT_ARCHIVE_BUCKET, archive_prefix + CATALOG_KEY)})
    client_fail._seed(DEFAULT_BUCKET, CATALOG_KEY, old_catalog)
    client_fail._seed(DEFAULT_BUCKET, SIG_KEY, old_sig)
    check(
        "swap_catalog: an archive-write failure raises rather than being swallowed",
        raises(Exception, lambda: swap_catalog(client_fail, DEFAULT_BUCKET, DEFAULT_ARCHIVE_BUCKET, new_catalog, new_sig)),
    )
    check(
        "swap_catalog: after an archive failure, the LIVE catalog.json is still the OLD bytes (never touched)",
        client_fail.store[(DEFAULT_BUCKET, CATALOG_KEY)]["data"] == old_catalog,
    )
    check(
        "swap_catalog: after an archive failure, no dest-bucket PUT/upload was ever attempted",
        all(not (c[1] == DEFAULT_BUCKET and c[0] in ("upload_file", "put_bytes")) for c in client_fail.calls),
    )

    return ok


# ---------------------------------------------------------------------
# CLI.
# ---------------------------------------------------------------------


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Publish catalog artifacts to cleophis-dist and archive-then-replace catalog.json/.sig.",
        epilog=(
            "Dry run (no credentials, no network — needs a local catalog.json + .sig, e.g. from\n"
            "build_catalog.py + sign_catalog.py, and the local artifact files it describes):\n"
            "  ./.venv/bin/python3 publish.py --dry-run\n\n"
            "Self-test (no credentials, no network, no local catalog needed):\n"
            "  ./.venv/bin/python3 publish.py --self-test\n\n"
            "Live publish, once B2 creds exist in .env:\n"
            "  ./.venv/bin/python3 publish.py"
        ),
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument(
        "--catalog",
        type=Path,
        default=DEFAULT_CATALOG,
        help=f"path to the signed catalog.json to publish; its <catalog>.sig sibling is read too (default: {DEFAULT_CATALOG})",
    )
    parser.add_argument(
        "--out-dir",
        type=Path,
        default=DEFAULT_OUT_DIR,
        help=f"local artifact root used to resolve each catalog artifact's file (default: {DEFAULT_OUT_DIR})",
    )
    parser.add_argument("--bucket", default=DEFAULT_BUCKET, help=f"destination bucket for artifacts + the live catalog (default: {DEFAULT_BUCKET})")
    parser.add_argument(
        "--archive-bucket", default=DEFAULT_ARCHIVE_BUCKET, help=f"bucket superseded catalogs are archived to (default: {DEFAULT_ARCHIVE_BUCKET})"
    )
    parser.add_argument(
        "--verify-pubkey",
        default=None,
        metavar="HEX",
        help="optional pre-flight: verify catalog.json.sig against catalog.json under this 64-hex-char ed25519 "
        "public key before publishing anything",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="print the full publish plan from local state only — loads no credentials, makes no network call, "
        "issues no HEAD/GET/PUT at all",
    )
    parser.add_argument(
        "--self-test",
        action="store_true",
        help="run the fake-client decision-logic self-test and exit (no --catalog/credentials/network needed)",
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)

    if args.self_test:
        return 0 if self_test() else 1

    try:
        catalog, catalog_bytes = load_catalog(args.catalog)
        sig_path = args.catalog.with_name(args.catalog.name + ".sig")
        sig_bytes = read_sig_exact(sig_path)

        if args.verify_pubkey is not None:
            if not verify_bytes(args.verify_pubkey, catalog_bytes, sig_bytes):
                raise PublishError(f"{sig_path} does NOT verify against {args.catalog} under the given --verify-pubkey — refusing to publish")
            print(f"[verify] {sig_path} verifies against {args.catalog} under the given public key", flush=True)

        plan: list[tuple[dict, Path]] = []
        for index, artifact in enumerate(catalog["artifacts"]):
            validate_artifact_entry(artifact, index)
            local_path = local_artifact_path(args.out_dir, artifact)
            verify_local_artifact(local_path, artifact)
            plan.append((artifact, local_path))
            print(
                f"[verify] {shlex.quote(str(local_path))} matches catalog sha256={artifact['sha256']} size={artifact['size']}",
                flush=True,
            )
    except PublishError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1

    if args.dry_run:
        print("[dry-run] no credentials loaded, no network touched — plan below is from local state only:", flush=True)
        for artifact, local_path in plan:
            print(
                f"[dry-run] would publish s3://{args.bucket}/{artifact['path']} <- {shlex.quote(str(local_path))} "
                f"(sha256={artifact['sha256']}, {artifact['size']} bytes; skipped if the remote object already has "
                "matching sha256 metadata, HARD ERROR if it exists with different/no metadata)",
                flush=True,
            )
        print(
            f"[dry-run] would then archive-then-replace s3://{args.bucket}/{CATALOG_KEY} "
            f"(new catalog_version={catalog['catalog_version']}): GET the current live catalog.json (its "
            "presence/state is unknown without network) — if present, archive it + its .sig to "
            f"s3://{args.archive_bucket}/archive/catalogs/v<old_version>/ FIRST, aborting before touching the "
            "live catalog on any archive failure; if absent, this is treated as the first-ever publish and "
            f"archiving is skipped; only then PUT the new {CATALOG_KEY} then {SIG_KEY}",
            flush=True,
        )
        print("[dry-run] OK — no objects were read or written", flush=True)
        return 0

    creds = load_b2_credentials(ENV_FILE)
    if creds is None:
        print(
            f"error: B2_ENDPOINT/B2_KEY_ID/B2_APP_KEY are not all set in {ENV_FILE} — see .env.example "
            "(or pass --dry-run to preview the plan without credentials)",
            file=sys.stderr,
        )
        return 1
    endpoint, key_id, app_key = creds
    client = S3Client(endpoint, key_id, app_key)

    try:
        for artifact, local_path in plan:
            result = publish_artifact(client, args.bucket, artifact, local_path)
            print(f"[publish] {result}: s3://{args.bucket}/{artifact['path']}", flush=True)

        swap_result = swap_catalog(client, args.bucket, args.archive_bucket, catalog_bytes, sig_bytes)
        print(f"[publish] catalog swap: {swap_result}", flush=True)
    except PublishError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1

    print("[done] publish complete", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
