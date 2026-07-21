#!/usr/bin/env python3
"""build_base.py — Task A2: base-model HF snapshot -> Q4_K_M GGUF.

Pipeline: `huggingface_hub` snapshot download (resumable, anonymous-capable)
-> `convert_hf_to_gguf.py` (f16 GGUF) -> `llama-quantize` (Q4_K_M) ->
streaming sha256 -> atomically-written `manifest.json`.

Env-dumb (see ../README.md "The env-dumb contract"): this script's only
inputs are its CLI flags below, `tools/pipeline/.env` (read ONLY for
`HF_TOKEN`, and only if that file exists), and the pinned toolchain
`setup_tools.sh` (Task A1) already set up at `tools/pipeline/.venv/` and
`tools/pipeline/llama.cpp/`. It never reads the calling shell's exported
environment and never assumes a working directory other than its own
`tools/pipeline/` root (all paths below are anchored on `__file__`).

Inputs:
    --repo         HF repo id                        (default: Qwen/Qwen3-4B)
    --revision     pinned HF commit/revision          (default: the commit
                    resolved for Qwen/Qwen3-4B's main branch when this
                    default was pinned — override to build a different one)
    --quant        llama-quantize type name           (default: Q4_K_M)
    --model-name   artifact/base_model identifier     (default: Qwen3-4B)
    --license      manifest `license` field           (default: Apache-2.0)
    --attribution  manifest `attribution` field        (default: Qwen Team,
                    Alibaba Cloud)
    --out-dir      final <model>/<quant>/ output root (default: work/out)
    --work-dir     scratch root for the HF snapshot
                   and intermediate f16 GGUF           (default: work/)
    tools/pipeline/.env: HF_TOKEN (optional; Qwen/Qwen3-4B is public, so
                    this script must — and does — work with no .env at all)

Outputs (under --out-dir, gitignored `work/` by default):
    <out-dir>/<model-name>/<quant>/<model-name>-Instruct-<quant>.gguf
    <out-dir>/<model-name>/<quant>/manifest.json

The intermediate f16 GGUF is kept (not deleted) under
<work-dir>/f16/<model-name>-f16.gguf — a later debug step needs an
adapter-on-f16 run.

Hashes: sha256 of the final quantized GGUF is computed by this script
itself via a streaming read (never trusted from elsewhere, never loads the
whole file into memory) and is what ends up in manifest.json.

Idempotent: every stage checks for its own completed output before doing
any work, so re-running after an interruption resumes rather than
redoing finished stages. Convert/quantize write to a `.partial` sibling
path and atomically rename on success, so a half-written output from a
killed run is never mistaken for a finished one.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import subprocess
import sys
import time
from datetime import datetime, timezone
from pathlib import Path

# The hf-xet fast-download backend is known-flaky (background writer channel
# errors), especially writing to slow/NTFS-backed mounts — force the
# classic HTTP downloader instead. setdefault (not direct assignment) so an
# operator who explicitly exported this can still override it; it must be
# set before `huggingface_hub` is imported anywhere below.
os.environ.setdefault("HF_HUB_DISABLE_XET", "1")

PIPELINE_ROOT = Path(__file__).resolve().parent

DEFAULT_REPO = "Qwen/Qwen3-4B"
# Resolved commit for Qwen/Qwen3-4B's main branch, pinned explicitly per
# the A2 task's binding decision (the hybrid thinking chat model, not
# -Base and not -Instruct-2507 — see task-A2-brief.md / task instructions).
DEFAULT_REVISION = "1cfa9a7208912126459214e8b04321603b3df60c"
DEFAULT_QUANT = "Q4_K_M"
DEFAULT_MODEL_NAME = "Qwen3-4B"
DEFAULT_LICENSE = "Apache-2.0"
DEFAULT_ATTRIBUTION = "Qwen Team, Alibaba Cloud (https://huggingface.co/Qwen/Qwen3-4B)"
DEFAULT_OUT_DIR = PIPELINE_ROOT / "work" / "out"
DEFAULT_WORK_DIR = PIPELINE_ROOT / "work"

CONVERT_SCRIPT = PIPELINE_ROOT / "llama.cpp" / "convert_hf_to_gguf.py"
QUANTIZE_BIN = PIPELINE_ROOT / "llama.cpp" / "bin" / "llama-quantize"
VENV_PYTHON = PIPELINE_ROOT / ".venv" / "bin" / "python3"
ENV_FILE = PIPELINE_ROOT / ".env"

HASH_CHUNK_SIZE = 1024 * 1024  # 1 MiB — stream the hash, never slurp the file.


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Build a quantized GGUF for a base model from a Hugging Face snapshot.",
    )
    parser.add_argument("--repo", default=DEFAULT_REPO, help=f"HF repo id (default: {DEFAULT_REPO})")
    parser.add_argument(
        "--revision",
        default=DEFAULT_REVISION,
        help=f"pinned HF commit/revision (default: {DEFAULT_REVISION})",
    )
    parser.add_argument("--quant", default=DEFAULT_QUANT, help=f"llama-quantize type name (default: {DEFAULT_QUANT})")
    parser.add_argument(
        "--model-name",
        default=DEFAULT_MODEL_NAME,
        help=f"artifact/base_model identifier used in paths and the manifest (default: {DEFAULT_MODEL_NAME})",
    )
    parser.add_argument("--license", default=DEFAULT_LICENSE, help=f"manifest license field (default: {DEFAULT_LICENSE})")
    parser.add_argument(
        "--attribution", default=DEFAULT_ATTRIBUTION, help=f"manifest attribution field (default: {DEFAULT_ATTRIBUTION})"
    )
    parser.add_argument("--out-dir", type=Path, default=DEFAULT_OUT_DIR, help=f"final output root (default: {DEFAULT_OUT_DIR})")
    parser.add_argument("--work-dir", type=Path, default=DEFAULT_WORK_DIR, help=f"scratch root (default: {DEFAULT_WORK_DIR})")
    return parser.parse_args(argv)


def load_hf_token(env_file: Path) -> str | None:
    """Read HF_TOKEN from tools/pipeline/.env, if that file exists and defines it.

    Qwen/Qwen3-4B is public, so the pipeline must work with no .env at all —
    this returns None rather than raising when the file is absent or the
    key is unset. Only this one file is consulted; the calling shell's
    exported environment is never read (see the env-dumb contract).
    """
    if not env_file.is_file():
        return None
    for raw_line in env_file.read_text().splitlines():
        line = raw_line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, _, value = line.partition("=")
        if key.strip() != "HF_TOKEN":
            continue
        value = value.strip().strip('"').strip("'")
        return value or None
    return None


def download_snapshot(
    repo: str,
    revision: str,
    work_dir: Path,
    token: str | None,
    max_attempts: int = 3,
) -> Path:
    """Resumable snapshot download of `repo`@`revision` into a fixed local dir.

    huggingface_hub resumes partially-downloaded files across calls as long
    as `local_dir`/`repo_id`/`revision` stay the same — files already
    complete on disk are skipped, partial ones resume from their
    `.incomplete` byte offset — so a bounded retry loop around the call
    is a safe way to ride out a transient failure (e.g. the hf-xet backend's
    "Background writer channel closed" error) without redoing finished work.
    """
    from huggingface_hub import snapshot_download

    dest = work_dir / "hf-snapshot" / repo.replace("/", "__")
    dest.mkdir(parents=True, exist_ok=True)
    print(f"[download] snapshot_download(repo={repo!r}, revision={revision!r}) -> {dest}", flush=True)

    last_exc: Exception | None = None
    for attempt in range(1, max_attempts + 1):
        try:
            snapshot_download(
                repo_id=repo,
                revision=revision,
                local_dir=dest,
                token=token,
                # Only the files convert_hf_to_gguf.py actually needs;
                # harmless if a repo has none of these (Qwen/Qwen3-4B ships
                # safetensors only).
                ignore_patterns=["*.msgpack", "*.h5", "*.pth", "*.bin", "*.onnx", "*.md", ".gitattributes"],
            )
            return dest
        except Exception as exc:  # noqa: BLE001 - deliberately broad: retry any transient download failure
            last_exc = exc
            print(f"[download] attempt {attempt}/{max_attempts} failed: {exc!r}", flush=True)
            if attempt < max_attempts:
                delay = 5 * attempt
                print(f"[download] retrying in {delay}s (resumes from files already on disk)...", flush=True)
                time.sleep(delay)

    raise RuntimeError(f"snapshot_download failed after {max_attempts} attempts") from last_exc


def convert_to_f16(snapshot_dir: Path, work_dir: Path, model_name: str) -> Path:
    """Run convert_hf_to_gguf.py to produce the intermediate f16 GGUF.

    Skips the (slow) conversion entirely if the final f16 file already
    exists from a prior run; otherwise converts to a `.partial` path and
    renames atomically on success so an interrupted run never leaves a
    half-written file at the final path.
    """
    f16_dir = work_dir / "f16"
    f16_dir.mkdir(parents=True, exist_ok=True)
    final = f16_dir / f"{model_name}-f16.gguf"
    if final.is_file() and final.stat().st_size > 0:
        print(f"[convert] {final} already exists, skipping", flush=True)
        return final

    tmp = final.with_name(final.name + ".partial")
    tmp.unlink(missing_ok=True)
    cmd = [
        str(VENV_PYTHON),
        str(CONVERT_SCRIPT),
        str(snapshot_dir),
        "--outtype",
        "f16",
        "--outfile",
        str(tmp),
    ]
    print(f"[convert] running: {' '.join(cmd)}", flush=True)
    subprocess.run(cmd, check=True)
    tmp.rename(final)
    return final


def quantize(f16_path: Path, out_gguf: Path, quant: str) -> Path:
    """Run llama-quantize to produce the final quantized GGUF.

    Same skip-if-done / write-to-.partial-then-rename idempotency pattern
    as convert_to_f16.
    """
    if out_gguf.is_file() and out_gguf.stat().st_size > 0:
        print(f"[quantize] {out_gguf} already exists, skipping", flush=True)
        return out_gguf

    out_gguf.parent.mkdir(parents=True, exist_ok=True)
    tmp = out_gguf.with_name(out_gguf.name + ".partial")
    tmp.unlink(missing_ok=True)
    cmd = [str(QUANTIZE_BIN), str(f16_path), str(tmp), quant]
    print(f"[quantize] running: {' '.join(cmd)}", flush=True)
    subprocess.run(cmd, check=True)
    tmp.rename(out_gguf)
    return out_gguf


def sha256_file(path: Path) -> str:
    """Streaming sha256 — never loads the whole (multi-GB) file into RAM."""
    digest = hashlib.sha256()
    with path.open("rb") as f:
        while True:
            chunk = f.read(HASH_CHUNK_SIZE)
            if not chunk:
                break
            digest.update(chunk)
    return digest.hexdigest()


def write_manifest_atomic(manifest_path: Path, manifest: dict) -> None:
    tmp = manifest_path.with_name(manifest_path.name + ".tmp")
    tmp.write_text(json.dumps(manifest, indent=2) + "\n")
    tmp.replace(manifest_path)  # atomic rename on POSIX, same filesystem


def build_manifest(
    *,
    basename: str,
    repo: str,
    revision: str,
    quant: str,
    sha256: str,
    size: int,
    license_id: str,
    attribution: str,
    model_name: str,
) -> dict:
    return {
        "name": basename,
        "source_repo": repo,
        "source_revision": revision,
        "quant": quant,
        "sha256": sha256,
        "size": size,
        "license": license_id,
        "attribution": attribution,
        "built_at": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "base_model": model_name,
        "kind": "base",
    }


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)

    if not CONVERT_SCRIPT.is_file():
        print(f"error: convert script not found at {CONVERT_SCRIPT} — run setup_tools.sh first", file=sys.stderr)
        return 1
    if not QUANTIZE_BIN.is_file():
        print(f"error: llama-quantize not found at {QUANTIZE_BIN} — run setup_tools.sh first", file=sys.stderr)
        return 1

    token = load_hf_token(ENV_FILE)

    basename = f"{args.model_name}-Instruct-{args.quant}.gguf"
    target_dir = args.out_dir / args.model_name / args.quant
    target_dir.mkdir(parents=True, exist_ok=True)
    final_gguf = target_dir / basename
    manifest_path = target_dir / "manifest.json"

    try:
        snapshot_dir = download_snapshot(args.repo, args.revision, args.work_dir, token)
        f16_path = convert_to_f16(snapshot_dir, args.work_dir, args.model_name)
        quantize(f16_path, final_gguf, args.quant)
    except (subprocess.CalledProcessError, RuntimeError) as exc:
        print(f"error: step failed: {exc}", file=sys.stderr)
        return 1

    print(f"[hash] sha256 of {final_gguf} ...", flush=True)
    digest = sha256_file(final_gguf)
    size = final_gguf.stat().st_size
    print(f"[hash] sha256={digest} size={size}", flush=True)

    manifest = build_manifest(
        basename=basename,
        repo=args.repo,
        revision=args.revision,
        quant=args.quant,
        sha256=digest,
        size=size,
        license_id=args.license,
        attribution=args.attribution,
        model_name=args.model_name,
    )
    write_manifest_atomic(manifest_path, manifest)
    print(f"[manifest] wrote {manifest_path}", flush=True)
    print(f"[done] {final_gguf} ({size} bytes, sha256={digest})", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
