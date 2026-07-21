#!/usr/bin/env python3
"""build_adapter.py — Task A3: PEFT LoRA adapter -> GGUF adapter.

Pipeline: B2 (S3-compatible) download of a PEFT adapter dir from the
PRIVATE `cleophis-models` bucket (or an already-local PEFT dir via
`--local-peft-dir`) -> `convert_lora_to_gguf.py` (applied against the same
pinned base-model HF snapshot Task A2 downloaded) -> streaming sha256 ->
atomically-written `manifest.json`.

Env-dumb (see ../README.md "The env-dumb contract"): this script's only
inputs are its CLI flags below, `tools/pipeline/.env` (read ONLY for
`B2_ENDPOINT`/`B2_KEY_ID`/`B2_APP_KEY`, and only when `--local-peft-dir`
isn't given), and the pinned toolchain `setup_tools.sh` (Task A1) already
set up at `tools/pipeline/.venv/` and `tools/pipeline/llama.cpp/`. It never
reads the calling shell's exported environment and never assumes a working
directory other than its own `tools/pipeline/` root (all paths below are
anchored on `__file__`).

Inputs:
    --bucket          B2 bucket holding the PEFT source     (default:
                       cleophis-models)
    --peft-prefix      key prefix of the PEFT dir in that bucket
                       (default: adapters/behavioral/v1/Qwen3-4B/)
    --local-peft-dir   skip the B2 fetch entirely and convert directly from
                       this already-local PEFT dir (also how this script is
                       exercised pre-handoff, before B2 read credentials
                       exist)                                (default: none)
    --base-dir         local HF snapshot dir for the base model this
                       adapter targets (passed to convert_lora_to_gguf.py's
                       `--base`)                              (default:
                       <work-dir>/hf-snapshot/<--base-repo with "/" ->
                       "__">, i.e. wherever build_base.py's A2 run already
                       put it)
    --base-repo        HF repo id of the base model — only used to compute
                       the default --base-dir above; not written to the
                       manifest                                (default:
                       Qwen/Qwen3-4B)
    --base-revision    pinned base-model commit/revision this adapter must
                       be applied to (recorded in the manifest and in the
                       provenance sidecar; NOT independently verified
                       against --base-dir's own contents — that's on the
                       operator to keep in sync with the A2 run that
                       produced --base-dir)                     (default:
                       the same commit build_base.py defaults to,
                       1cfa9a7208912126459214e8b04321603b3df60c)
    --model-name       base_model identifier used in paths/manifest
                       (default: Qwen3-4B)
    --adapter-name     adapter identifier; combined with --model-name to
                       form the fixed cross-track basename
                       "<adapter-name>-<model-name>.gguf"       (default:
                       behavioral-v1)
    --contract-version prompt-contract identifier for the manifest. If the
                       PEFT source directory carries its own manifest with
                       a `contract_version` field, THAT wins unless this
                       flag is explicitly passed (explicit CLI always beats
                       a discovered value)                       (default:
                       unset -> falls back to the source manifest's value,
                       then to "prompt-contract-v0")
    --outtype          convert_lora_to_gguf.py --outtype passthrough
                       (choices: f32, f16, bf16, q8_0, auto)      (default:
                       f16 — LoRA deltas are tiny, f16 keeps effectively
                       full fidelity at half the f32 size)
    --out-dir          final <model>/adapter/ output root         (default:
                       work/out)
    --work-dir         scratch root for the downloaded PEFT dir    (default:
                       work/)
    --dry-run          still fully resolves the PEFT source — including a
                       REAL B2 fetch when --local-peft-dir isn't given;
                       --dry-run does NOT stub that out — and prints the
                       exact convert_lora_to_gguf.py command that would
                       run; only the conversion itself (and the
                       hash/manifest steps after it) is skipped, so no
                       output is written (orchestration smoke test — see
                       Task A3's pre-handoff verification note)
    tools/pipeline/.env: B2_ENDPOINT, B2_KEY_ID, B2_APP_KEY (required
                    unless --local-peft-dir is given)

Outputs (under --out-dir, gitignored `work/` by default):
    <out-dir>/<model-name>/adapter/<adapter-name>-<model-name>.gguf
    <out-dir>/<model-name>/adapter/<adapter-name>-<model-name>.gguf.provenance.json
    <out-dir>/<model-name>/adapter/manifest.json

The downloaded PEFT source is kept (not deleted) under
<work-dir>/peft/<model-name>/ as lineage, mirroring the f16 GGUF
build_base.py keeps under <work-dir>/f16/.

Hashes: sha256 of the final adapter GGUF is computed by this script itself
via a streaming read (never trusted from elsewhere, never loads the whole
file into memory) and is what ends up in manifest.json.

Idempotent: the B2 download is a per-file skip check that compares size
AND, when the object's ETag is a plain (single-part-upload) MD5 — B2/S3
multipart-upload ETags are a `"<hex>-<parts>"` composite, not the object's
MD5, and can't be verified without replicating their multipart hash
algorithm — the local file's own MD5 against that ETag; a multipart ETag
(or a size mismatch, or no local file yet) always (re)downloads rather
than trusting size alone. Cheap for a PEFT dir's handful of small files.
Nothing already on disk is ever deleted or overwritten in place (each file
lands via `.partial` temp-then-rename).

The GGUF conversion follows the exact same provenance-sidecar pattern
build_base.py's convert/quantize stages use: convert writes to a
`.partial` sibling and renames atomically on success, and a re-run only
treats an existing output as complete if its `<output>.provenance.json`
sidecar matches THIS invocation's (source, base_revision,
peft_content_sha256). That third key is deliberate: `source` alone (an S3
prefix, or a local path) is NOT content-addressed the way A2's pinned git
`revision` is — a mutable bucket prefix can be silently overwritten by a
retrain, and a same-path/same-revision re-run would otherwise match a
stale sidecar and skip, serving the OLD adapter bytes into A4's catalog.
`peft_content_sha256` (see `peft_content_sha256_of_dir()`) is a
deterministic hash of every file under the resolved PEFT dir, so an
in-place retrain at the same `source` changes the key and is caught. A
same-path output whose sidecar doesn't match on any of the three fields is
provenance-untrustworthy for the signed catalog (A4 trusts the manifest
this script writes), so a mismatch is a hard refusal (StaleArtifactError),
never a silent rebuild or silent reuse — the operator has to explicitly
clear the stale output + its sidecar (and any downstream manifest) before
re-running. `peft_content_sha256` is also carried into the output
manifest itself, so A4 gets this provenance for free without re-deriving
it.

PEFT source manifest carry-forward: the bucket PEFT dir's own manifest
format is not fixed by this script. If a `manifest.json` (or, failing
that, some other `*.json` that isn't `adapter_config.json`) is found in
the PEFT dir, it's parsed and, if it's a JSON object, carried forward
VERBATIM into our own manifest under the `source_manifest` key; its
absence is not an error. Its `contract_version` field (if a string) also
becomes this script's default --contract-version, subject to being
overridden by an explicit --contract-version CLI flag.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shlex
import subprocess
import sys
import time
from datetime import datetime, timezone
from pathlib import Path

PIPELINE_ROOT = Path(__file__).resolve().parent

DEFAULT_BUCKET = "cleophis-models"
DEFAULT_PEFT_PREFIX = "adapters/behavioral/v1/Qwen3-4B/"
DEFAULT_BASE_REPO = "Qwen/Qwen3-4B"
# Same pinned commit build_base.py (Task A2) defaults to — the adapter must
# be applied to the same base revision the shipped base GGUF was built
# from. Kept as an independent constant (not imported from build_base.py)
# so this script stays a standalone, env-dumb unit; if A2's pin ever moves,
# bump both constants together, deliberately.
DEFAULT_BASE_REVISION = "1cfa9a7208912126459214e8b04321603b3df60c"
DEFAULT_MODEL_NAME = "Qwen3-4B"
DEFAULT_ADAPTER_NAME = "behavioral-v1"
DEFAULT_CONTRACT_VERSION = "prompt-contract-v0"
DEFAULT_OUTTYPE = "f16"
DEFAULT_OUT_DIR = PIPELINE_ROOT / "work" / "out"
DEFAULT_WORK_DIR = PIPELINE_ROOT / "work"

CONVERT_LORA_SCRIPT = PIPELINE_ROOT / "llama.cpp" / "convert_lora_to_gguf.py"
VENV_PYTHON = PIPELINE_ROOT / ".venv" / "bin" / "python3"
ENV_FILE = PIPELINE_ROOT / ".env"

HASH_CHUNK_SIZE = 1024 * 1024  # 1 MiB — stream the hash, never slurp the file.

# adapter_config.json is PEFT-internal config, not a source-manifest
# candidate even though it's a *.json file in the PEFT dir.
PEFT_NON_MANIFEST_FILENAMES = {"adapter_config.json"}


class StaleArtifactError(RuntimeError):
    """An on-disk output exists but its provenance sidecar doesn't match the
    requested (source, base_revision, peft_content_sha256) — refuse rather
    than silently rebuild/reuse it. Mirrors build_base.py's
    StaleArtifactError, plus the content-hash key `source` alone can't
    provide (see the module docstring's "Idempotent" section for why)."""


def provenance_path(artifact_path: Path) -> Path:
    return artifact_path.with_name(artifact_path.name + ".provenance.json")


def read_provenance(artifact_path: Path) -> dict | None:
    """Read an artifact's provenance sidecar; None if absent or unparseable
    (an unparseable sidecar is treated the same as a missing one — both fail
    the equality check in `check_provenance_or_refuse` and refuse)."""
    path = provenance_path(artifact_path)
    if not path.is_file():
        return None
    try:
        return json.loads(path.read_text())
    except (json.JSONDecodeError, OSError):
        return None


def write_provenance_atomic(artifact_path: Path, source: str, base_revision: str, peft_content_sha256: str) -> None:
    path = provenance_path(artifact_path)
    tmp = path.with_name(path.name + ".tmp")
    tmp.write_text(
        json.dumps(
            {"source": source, "base_revision": base_revision, "peft_content_sha256": peft_content_sha256},
            indent=2,
        )
        + "\n"
    )
    tmp.replace(path)  # atomic rename on POSIX, same filesystem


def check_provenance_or_refuse(artifact_path: Path, source: str, base_revision: str, peft_content_sha256: str) -> bool:
    """True if `artifact_path` already exists AND its provenance sidecar
    matches (source, base_revision, peft_content_sha256) — conversion can
    be skipped. False if the path doesn't exist yet (conversion should run
    normally). Raises StaleArtifactError if the path exists but the
    sidecar is missing/mismatched, rather than silently rebuilding or
    silently trusting a possibly-wrong artifact.

    peft_content_sha256 is the key that actually pins this to PEFT BYTES:
    `source` (an S3 prefix or local path) is mutable — an in-place retrain
    at the same bucket path, followed by a default-args re-run, would
    otherwise match on (source, base_revision) alone and silently keep
    serving the OLD converted adapter into A4's catalog.
    """
    if not (artifact_path.is_file() and artifact_path.stat().st_size > 0):
        return False
    expected = {"source": source, "base_revision": base_revision, "peft_content_sha256": peft_content_sha256}
    actual = read_provenance(artifact_path)
    if actual == expected:
        return True
    raise StaleArtifactError(
        f"{artifact_path} already exists but its provenance sidecar "
        f"({provenance_path(artifact_path).name}) is {actual!r}, not the "
        f"requested {expected!r}. Refusing to silently reuse or overwrite a "
        f"potentially mismatched artifact. Remove {artifact_path.name} and "
        f"{provenance_path(artifact_path).name} (and any downstream "
        f"manifest.json built from it) if you intend to rebuild for this "
        f"source/base_revision/PEFT content."
    )


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Convert a PEFT LoRA adapter (from B2 or a local dir) to a GGUF adapter, applied to a pinned base-model revision.",
    )
    parser.add_argument("--bucket", default=DEFAULT_BUCKET, help=f"B2 bucket holding the PEFT source (default: {DEFAULT_BUCKET})")
    parser.add_argument(
        "--peft-prefix",
        default=DEFAULT_PEFT_PREFIX,
        help=f"key prefix of the PEFT dir in that bucket (default: {DEFAULT_PEFT_PREFIX})",
    )
    parser.add_argument(
        "--local-peft-dir",
        type=Path,
        default=None,
        help="skip the B2 fetch and convert directly from this already-local PEFT dir",
    )
    parser.add_argument(
        "--base-dir",
        type=Path,
        default=None,
        help="local HF snapshot dir for the base model (default: <work-dir>/hf-snapshot/<--base-repo with '/' -> '__'>)",
    )
    parser.add_argument(
        "--base-repo",
        default=DEFAULT_BASE_REPO,
        help=f"HF repo id of the base model, used only to compute the default --base-dir (default: {DEFAULT_BASE_REPO})",
    )
    parser.add_argument(
        "--base-revision",
        default=DEFAULT_BASE_REVISION,
        help=f"pinned base-model commit/revision this adapter targets (default: {DEFAULT_BASE_REVISION})",
    )
    parser.add_argument(
        "--model-name",
        default=DEFAULT_MODEL_NAME,
        help=f"artifact/base_model identifier used in paths and the manifest (default: {DEFAULT_MODEL_NAME})",
    )
    parser.add_argument(
        "--adapter-name",
        default=DEFAULT_ADAPTER_NAME,
        help=f"adapter identifier, combined with --model-name for the output basename (default: {DEFAULT_ADAPTER_NAME})",
    )
    parser.add_argument(
        "--contract-version",
        default=None,
        help="manifest contract_version (default: the PEFT source manifest's contract_version if present, else "
        f"{DEFAULT_CONTRACT_VERSION!r})",
    )
    parser.add_argument(
        "--outtype",
        default=DEFAULT_OUTTYPE,
        choices=["f32", "f16", "bf16", "q8_0", "auto"],
        help=f"convert_lora_to_gguf.py --outtype passthrough (default: {DEFAULT_OUTTYPE})",
    )
    parser.add_argument("--out-dir", type=Path, default=DEFAULT_OUT_DIR, help=f"final output root (default: {DEFAULT_OUT_DIR})")
    parser.add_argument("--work-dir", type=Path, default=DEFAULT_WORK_DIR, help=f"scratch root (default: {DEFAULT_WORK_DIR})")
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="still fully resolves the PEFT source (a REAL B2 fetch happens if --local-peft-dir isn't given) but only "
        "prints the convert_lora_to_gguf.py command instead of running it, and writes no output",
    )
    return parser.parse_args(argv)


def load_env_file(env_file: Path) -> dict[str, str]:
    """Parse tools/pipeline/.env into a dict; {} if the file doesn't exist.

    Only this one file is consulted; the calling shell's exported
    environment is never read (see the env-dumb contract).
    """
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
    missing/unset (the caller decides whether that's fatal — it isn't when
    --local-peft-dir makes the B2 fetch unnecessary)."""
    values = load_env_file(env_file)
    endpoint = values.get("B2_ENDPOINT") or None
    key_id = values.get("B2_KEY_ID") or None
    app_key = values.get("B2_APP_KEY") or None
    if not (endpoint and key_id and app_key):
        return None
    return endpoint, key_id, app_key


def md5_file(path: Path) -> str:
    """Streaming MD5 of a local file — used ONLY to compare against a
    B2/S3 object's plain-upload ETag in the download skip-check below.
    This is not a security use: B2/S3's own ETag mechanics are what pin
    this to MD5, not a choice made here. `usedforsecurity=False` avoids a
    spurious failure on Python builds that enforce FIPS-mode hash
    restrictions for anything not explicitly marked non-security.
    """
    digest = hashlib.md5(usedforsecurity=False)
    with path.open("rb") as f:
        while True:
            chunk = f.read(HASH_CHUNK_SIZE)
            if not chunk:
                break
            digest.update(chunk)
    return digest.hexdigest()


def is_plain_md5_etag(etag: str) -> bool:
    """True if `etag` (already stripped of surrounding quotes) looks like a
    plain single-part-upload MD5 hex digest, as opposed to a multipart-
    upload composite ETag (`"<hex>-<num_parts>"`). A composite ETag is NOT
    the MD5 of the object's bytes and can't be verified without
    replicating B2/S3's multipart hashing algorithm, so callers should
    treat a non-plain-MD5 ETag as "can't verify, must (re)download"."""
    return bool(etag) and "-" not in etag and len(etag) == 32


def download_peft_dir_from_s3(
    bucket: str,
    prefix: str,
    dest_dir: Path,
    endpoint: str,
    key_id: str,
    app_key: str,
    max_attempts: int = 3,
) -> Path:
    """Download every object under s3://{bucket}/{prefix} into dest_dir,
    preserving the relative key layout.

    Resumable in the simple sense the brief calls for (small PEFT files):
    a file already on disk is skipped only if its size matches the remote
    object's size AND (when the object's ETag is a plain single-part-
    upload MD5 — see `is_plain_md5_etag`) the local file's own MD5 also
    matches that ETag. A multipart-upload ETag (or a size mismatch, or no
    local file yet) always (re)downloads rather than trusting size alone —
    cheap for a PEFT dir's handful of small files, and it closes the gap
    where an in-place file overwrite at the same key/size would otherwise
    silently pass a size-only check. Nothing already on disk is ever
    deleted or overwritten in place (each file lands via a `.partial`
    temp-then-rename so a killed download never masquerades as complete).
    Kept as lineage — this function has no delete path at all.
    """
    import boto3

    client = boto3.client(
        "s3",
        endpoint_url=endpoint,
        aws_access_key_id=key_id,
        aws_secret_access_key=app_key,
    )

    prefix = prefix if prefix.endswith("/") else prefix + "/"
    dest_dir.mkdir(parents=True, exist_ok=True)

    print(f"[s3] listing s3://{bucket}/{prefix}", flush=True)
    objects: list[dict] = []
    last_exc: Exception | None = None
    for attempt in range(1, max_attempts + 1):
        try:
            objects = []
            paginator = client.get_paginator("list_objects_v2")
            for page in paginator.paginate(Bucket=bucket, Prefix=prefix):
                objects.extend(page.get("Contents", []))
            break
        except Exception as exc:  # noqa: BLE001 - deliberately broad: retry any transient listing failure
            last_exc = exc
            print(f"[s3] list attempt {attempt}/{max_attempts} failed: {exc!r}", flush=True)
            if attempt < max_attempts:
                delay = 5 * attempt
                print(f"[s3] retrying in {delay}s...", flush=True)
                time.sleep(delay)
    else:
        raise RuntimeError(f"listing s3://{bucket}/{prefix} failed after {max_attempts} attempts") from last_exc

    if not objects:
        raise RuntimeError(f"no objects found at s3://{bucket}/{prefix} — check the bucket/prefix and credentials")

    for obj in objects:
        key = obj["Key"]
        if key.endswith("/"):
            continue  # S3 "directory marker" placeholder, not a real file
        rel = key[len(prefix):]
        if not rel:
            continue
        local_path = dest_dir / rel
        local_path.parent.mkdir(parents=True, exist_ok=True)
        remote_size = obj["Size"]
        etag = obj.get("ETag", "").strip('"')
        if local_path.is_file() and local_path.stat().st_size == remote_size:
            if is_plain_md5_etag(etag):
                if md5_file(local_path) == etag:
                    print(f"[s3] {rel} already present, size+MD5-verified ({remote_size} bytes), skipping", flush=True)
                    continue
                print(f"[s3] {rel} present locally but its MD5 doesn't match the remote ETag — redownloading", flush=True)
            else:
                print(
                    f"[s3] {rel} present locally with matching size but a non-plain-MD5 ETag "
                    "(multipart upload) — can't verify content from ETag alone, redownloading",
                    flush=True,
                )

        last_exc: Exception | None = None
        for attempt in range(1, max_attempts + 1):
            tmp = local_path.with_name(local_path.name + ".partial")
            try:
                print(f"[s3] downloading s3://{bucket}/{key} -> {local_path} (attempt {attempt}/{max_attempts})", flush=True)
                client.download_file(bucket, key, str(tmp))
                tmp.rename(local_path)
                break
            except Exception as exc:  # noqa: BLE001 - deliberately broad: retry any transient download failure
                last_exc = exc
                print(f"[s3] attempt {attempt}/{max_attempts} for {key} failed: {exc!r}", flush=True)
                if attempt < max_attempts:
                    delay = 5 * attempt
                    print(f"[s3] retrying in {delay}s...", flush=True)
                    time.sleep(delay)
        else:
            raise RuntimeError(f"download of s3://{bucket}/{key} failed after {max_attempts} attempts") from last_exc

    return dest_dir


def find_source_manifest(peft_dir: Path) -> dict | None:
    """Look for a manifest the PEFT source dir itself carries.

    Prefers `manifest.json`; failing that, the first other `*.json` file
    that isn't `adapter_config.json` (PEFT's own config, not a manifest).
    Returns the parsed JSON object verbatim, or None if nothing manifest-
    shaped is found or what's found doesn't parse as a JSON object — either
    way, absence is not an error (per Task A3's binding decision: the real
    bucket manifest format is unknown until handoff).
    """
    candidate = peft_dir / "manifest.json"
    if not candidate.is_file():
        others = sorted(
            p for p in peft_dir.glob("*.json") if p.name not in PEFT_NON_MANIFEST_FILENAMES and p.name != "manifest.json"
        )
        candidate = others[0] if others else None

    if candidate is None:
        return None

    try:
        data = json.loads(candidate.read_text())
    except (json.JSONDecodeError, OSError) as exc:
        print(f"[manifest] warning: found {candidate} but couldn't parse it ({exc!r}); ignoring", file=sys.stderr)
        return None

    if not isinstance(data, dict):
        print(f"[manifest] warning: {candidate} did not contain a JSON object; ignoring", file=sys.stderr)
        return None

    print(f"[manifest] carrying forward source manifest from {candidate}", flush=True)
    return data


def resolve_contract_version(cli_value: str | None, source_manifest: dict | None) -> str:
    """Explicit --contract-version always wins; otherwise the source
    manifest's contract_version (if it's a non-empty string); otherwise
    DEFAULT_CONTRACT_VERSION."""
    if cli_value:
        return cli_value
    if source_manifest is not None:
        value = source_manifest.get("contract_version")
        if isinstance(value, str) and value:
            return value
    return DEFAULT_CONTRACT_VERSION


def convert_lora(
    peft_dir: Path,
    base_dir: Path,
    out_gguf: Path,
    outtype: str,
    source: str,
    base_revision: str,
    peft_content_sha256: str,
    dry_run: bool = False,
) -> Path:
    """Run convert_lora_to_gguf.py to produce the adapter GGUF.

    Same provenance-checked skip-if-done / write-to-.partial-then-rename
    idempotency pattern as build_base.py's convert_to_f16/quantize, keyed
    on (source, base_revision, peft_content_sha256) — see
    `check_provenance_or_refuse`'s docstring for why the content hash is
    load-bearing, not just belt-and-suspenders. When dry_run is True, the
    provenance short-circuit is skipped (a dry run always builds and
    prints the command) and the function returns before invoking the
    subprocess or touching the output path at all.
    """
    if not dry_run and check_provenance_or_refuse(out_gguf, source, base_revision, peft_content_sha256):
        print(f"[convert] {out_gguf} already exists with matching provenance, skipping", flush=True)
        return out_gguf

    out_gguf.parent.mkdir(parents=True, exist_ok=True)
    tmp = out_gguf.with_name(out_gguf.name + ".partial")
    cmd = [
        str(VENV_PYTHON),
        str(CONVERT_LORA_SCRIPT),
        str(peft_dir),
        "--base",
        str(base_dir),
        "--outtype",
        outtype,
        "--outfile",
        str(tmp),
    ]
    print(f"[convert] {'would run' if dry_run else 'running'}: {shlex.join(cmd)}", flush=True)
    if dry_run:
        return out_gguf

    tmp.unlink(missing_ok=True)
    subprocess.run(cmd, check=True)
    tmp.rename(out_gguf)
    write_provenance_atomic(out_gguf, source, base_revision, peft_content_sha256)
    return out_gguf


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


def peft_content_sha256_of_dir(peft_dir: Path) -> str:
    """Deterministic content hash of an entire PEFT dir: every file under
    it, sorted by relative POSIX path for determinism, each paired with
    its own streaming sha256, folded into one aggregate sha256.

    This is what actually pins provenance to PEFT BYTES. Unlike A2's
    `--revision` (an immutable pinned git commit), A3's `--peft-prefix` is
    a MUTABLE B2 key — an in-place retrain published to the same prefix,
    followed by a default-args re-run, would match a provenance sidecar
    keyed only on (source, base_revision) and silently keep serving the
    OLD converted adapter into A4's catalog. Hashing the resolved dir's
    actual bytes (not just its location) closes that gap: any changed
    byte in any file changes this hash, so a stale sidecar is caught by
    `check_provenance_or_refuse` instead of silently matching.

    `.partial`/`.tmp` residue (shouldn't exist after a successful
    download, but defensively excluded) is skipped so an interrupted
    prior download can't perturb the hash of files that already are
    complete.
    """
    digest = hashlib.sha256()
    paths = sorted(
        p for p in peft_dir.rglob("*") if p.is_file() and p.suffix not in (".partial", ".tmp")
    )
    for path in paths:
        rel = path.relative_to(peft_dir).as_posix()
        digest.update(rel.encode("utf-8"))
        digest.update(b"\0")
        digest.update(sha256_file(path).encode("ascii"))
        digest.update(b"\n")
    return digest.hexdigest()


def write_manifest_atomic(manifest_path: Path, manifest: dict) -> None:
    tmp = manifest_path.with_name(manifest_path.name + ".tmp")
    tmp.write_text(json.dumps(manifest, indent=2) + "\n")
    tmp.replace(manifest_path)  # atomic rename on POSIX, same filesystem


def build_manifest(
    *,
    basename: str,
    source: str,
    peft_content_sha256: str,
    base_model: str,
    base_revision: str,
    sha256: str,
    size: int,
    contract_version: str,
    source_manifest: dict | None,
) -> dict:
    return {
        "name": basename,
        "source": source,
        # Content hash of the PEFT source dir this adapter was converted
        # from — NOT the same as `sha256` (that's the converted GGUF's own
        # hash). Carried forward from the provenance sidecar so A4 gets it
        # for free; see check_provenance_or_refuse's docstring for why
        # `source` alone (a mutable B2 prefix) can't provide this guarantee.
        "peft_content_sha256": peft_content_sha256,
        "base_model": base_model,
        "base_revision": base_revision,
        "kind": "adapter",
        "sha256": sha256,
        "size": size,
        "contract_version": contract_version,
        "built_at": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "source_manifest": source_manifest,
    }


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)

    if not CONVERT_LORA_SCRIPT.is_file():
        print(f"error: convert script not found at {CONVERT_LORA_SCRIPT} — run setup_tools.sh first", file=sys.stderr)
        return 1

    # --- Resolve the PEFT source dir ------------------------------------
    if args.local_peft_dir is not None:
        peft_dir = args.local_peft_dir
        if not peft_dir.is_dir():
            print(f"error: --local-peft-dir {peft_dir} is not a directory", file=sys.stderr)
            return 1
        source = str(peft_dir)
    else:
        creds = load_b2_credentials(ENV_FILE)
        if creds is None:
            print(
                "error: B2_ENDPOINT/B2_KEY_ID/B2_APP_KEY are not all set in "
                f"{ENV_FILE} — either fill in .env (see .env.example) or pass "
                "--local-peft-dir to convert from an already-local PEFT dir",
                file=sys.stderr,
            )
            return 1
        endpoint, key_id, app_key = creds
        dest = args.work_dir / "peft" / args.model_name
        try:
            peft_dir = download_peft_dir_from_s3(args.bucket, args.peft_prefix, dest, endpoint, key_id, app_key)
        except RuntimeError as exc:
            print(f"error: PEFT download failed: {exc}", file=sys.stderr)
            return 1
        source = f"s3://{args.bucket}/{args.peft_prefix.rstrip('/')}/"

    adapter_config = peft_dir / "adapter_config.json"
    if not adapter_config.is_file():
        print(f"error: {peft_dir} does not look like a PEFT adapter dir (missing adapter_config.json)", file=sys.stderr)
        return 1
    if not (peft_dir / "adapter_model.safetensors").is_file() and not (peft_dir / "adapter_model.bin").is_file():
        print(
            f"error: {peft_dir} does not look like a PEFT adapter dir "
            "(missing adapter_model.safetensors and adapter_model.bin)",
            file=sys.stderr,
        )
        return 1

    # Content-address the resolved PEFT dir's actual bytes — `source` alone
    # (an S3 prefix or local path) is mutable and can't detect an in-place
    # retrain published to the same location; see peft_content_sha256_of_dir's
    # docstring.
    peft_content_sha256 = peft_content_sha256_of_dir(peft_dir)
    print(f"[hash] peft_content_sha256({peft_dir}) = {peft_content_sha256}", flush=True)

    # --- Resolve the base model dir --------------------------------------
    base_dir = args.base_dir if args.base_dir is not None else args.work_dir / "hf-snapshot" / args.base_repo.replace("/", "__")
    if not (base_dir / "config.json").is_file():
        print(
            f"error: base model dir {base_dir} has no config.json — run build_base.py "
            "first (or pass --base-dir pointing at a local HF snapshot for the base model)",
            file=sys.stderr,
        )
        return 1

    source_manifest = find_source_manifest(peft_dir)
    contract_version = resolve_contract_version(args.contract_version, source_manifest)

    basename = f"{args.adapter_name}-{args.model_name}.gguf"
    target_dir = args.out_dir / args.model_name / "adapter"
    target_dir.mkdir(parents=True, exist_ok=True)
    final_gguf = target_dir / basename
    manifest_path = target_dir / "manifest.json"

    try:
        convert_lora(
            peft_dir,
            base_dir,
            final_gguf,
            args.outtype,
            source,
            args.base_revision,
            peft_content_sha256,
            dry_run=args.dry_run,
        )
    except (subprocess.CalledProcessError, RuntimeError) as exc:
        print(f"error: conversion failed: {exc}", file=sys.stderr)
        return 1

    if args.dry_run:
        print("[dry-run] stopping before hashing/manifest — no output was written", flush=True)
        return 0

    print(f"[hash] sha256 of {final_gguf} ...", flush=True)
    digest = sha256_file(final_gguf)
    size = final_gguf.stat().st_size
    print(f"[hash] sha256={digest} size={size}", flush=True)

    manifest = build_manifest(
        basename=basename,
        source=source,
        peft_content_sha256=peft_content_sha256,
        base_model=args.model_name,
        base_revision=args.base_revision,
        sha256=digest,
        size=size,
        contract_version=contract_version,
        source_manifest=source_manifest,
    )
    write_manifest_atomic(manifest_path, manifest)
    print(f"[manifest] wrote {manifest_path}", flush=True)
    print(f"[done] {final_gguf} ({size} bytes, sha256={digest})", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
