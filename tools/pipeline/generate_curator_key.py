#!/usr/bin/env python3
"""generate_curator_key.py — Task A4 (controller addition): mint the
curator ed25519 keypair. Run ONCE, by a human, at handoff — not part of
any other script's automated pipeline.

Env-dumb (see README.md "The env-dumb contract"): this script's only input
is its one CLI flag, `--out`. It reads no `.env`, no other pipeline state,
and never assumes a working directory other than its own `tools/pipeline/`
root (used only to compute this repo's root for the "outside the repo"
check below).

Inputs:
    --out <path>   REQUIRED. Where to write the 32-byte seed, hex-encoded,
                   one line, mode 0600.
                     - MUST resolve to a location OUTSIDE this repository's
                       working tree. Refused otherwise: a private key must
                       never be writable to a path a later `git add` could
                       ever pick up, even by accident.
                     - MUST NOT already exist (neither <out> nor <out>.pub).
                       Refused otherwise: this script never overwrites an
                       existing key.

Outputs:
    <out>          the 32-byte ed25519 seed, 64 lowercase hex chars + a
                   trailing newline, mode 0600. This is the file
                   `CURATOR_KEY_FILE` in tools/pipeline/.env should point
                   at.
    <out>.pub      the CORRESPONDING 32-byte public key, 64 lowercase hex
                   chars + a trailing newline, mode 0644 — NOT secret; safe
                   to keep alongside the key file, back up, or share.

stdout: ONLY ever prints the PUBLIC key, in exactly two forms, on two
lines — nothing else goes to stdout, so it's safe to pipe/copy directly:
    1. 64 lowercase hex chars
    2. a ready-to-paste Rust literal `Some([0x.., 0x.., ...])`, meant to
       replace `crates/kpack-core/src/sign.rs`'s
       `CURATOR_PUBLIC_KEY: Option<[u8; 32]> = None` at the §2.6
       sign-and-publish milestone.
Status/progress messages (file paths written, reminders, errors) go to
stderr instead, so they never contaminate stdout's copy-paste output.

The private seed is NEVER printed, logged, or written anywhere other than
<out>.
"""

from __future__ import annotations

import argparse
import os
import sys
from pathlib import Path

from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
from cryptography.hazmat.primitives.serialization import Encoding, NoEncryption, PrivateFormat, PublicFormat

PIPELINE_ROOT = Path(__file__).resolve().parent
REPO_ROOT = PIPELINE_ROOT.parent.parent  # tools/pipeline -> tools -> repo root


class KeygenError(RuntimeError):
    """Any refusal — path inside the repo, existing key file, or an I/O
    failure while writing. Always caught at main()'s top level and
    reported as a clean one-line error, never a raw traceback."""


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Generate the curator ed25519 keypair used to sign catalog.json. Run once, by a human, outside the repo.",
    )
    parser.add_argument(
        "--out",
        type=Path,
        required=True,
        help="path to write the 32-byte seed (hex) to; must be OUTSIDE the repo and must not already exist",
    )
    return parser.parse_args(argv)


def check_outside_repo(out_path: Path) -> Path:
    """Resolves `out_path` and refuses (KeygenError) if it's inside this
    repository's working tree (equal to, or nested under, REPO_ROOT)."""
    resolved = out_path.expanduser().resolve()
    repo_root_resolved = REPO_ROOT.resolve()
    if resolved == repo_root_resolved or resolved.is_relative_to(repo_root_resolved):
        raise KeygenError(
            f"--out {out_path} resolves to {resolved}, which is INSIDE this repository ({repo_root_resolved}) — "
            "the curator private key must never live anywhere a `git add` could ever pick it up. Choose a path "
            "outside the repo, e.g. ~/.cleophis/curator_ed25519.key"
        )
    return resolved


def write_seed_file(path: Path, seed_hex: str) -> None:
    """Writes `seed_hex` to `path` with mode 0600 from the very first
    write: `os.open` with O_EXCL so there is no window where the file
    exists with looser default permissions before a subsequent chmod, and
    O_EXCL is also an atomic second guard against a racing overwrite of an
    existing file (belt-and-suspenders on top of the caller's own
    pre-check)."""
    path.parent.mkdir(parents=True, exist_ok=True)
    try:
        fd = os.open(str(path), os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    except FileExistsError as exc:
        raise KeygenError(f"{path} already exists — refusing to overwrite an existing key file.") from exc
    except OSError as exc:
        raise KeygenError(f"could not create {path}: {exc}") from exc
    try:
        with os.fdopen(fd, "w") as f:
            f.write(seed_hex + "\n")
    except OSError as exc:
        path.unlink(missing_ok=True)
        raise KeygenError(f"failed writing {path}: {exc}") from exc
    os.chmod(path, 0o600)  # belt-and-suspenders: umask can't have loosened the O_CREAT mode above, but be explicit


def write_pub_file(path: Path, pubkey_hex: str) -> Path:
    """Writes the (non-secret) public key hex to `<path>.pub`, mode 0644,
    via the same atomic tmp-then-rename pattern the rest of this pipeline
    uses. Refuses if `<path>.pub` already exists."""
    pub_path = path.with_name(path.name + ".pub")
    if pub_path.exists():
        raise KeygenError(f"{pub_path} already exists — refusing to overwrite it.")
    tmp = pub_path.with_name(pub_path.name + ".tmp")
    tmp.write_text(pubkey_hex + "\n")
    os.chmod(tmp, 0o644)
    tmp.replace(pub_path)  # atomic rename on POSIX, same filesystem
    return pub_path


def rust_literal(pubkey: bytes) -> str:
    hex_bytes = ", ".join(f"0x{b:02x}" for b in pubkey)
    return f"Some([{hex_bytes}])"


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)

    try:
        out_path = check_outside_repo(args.out)
    except KeygenError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1

    pub_path = out_path.with_name(out_path.name + ".pub")
    if out_path.exists():
        print(f"error: {out_path} already exists — refusing to overwrite an existing key file. Choose a different --out path.", file=sys.stderr)
        return 1
    if pub_path.exists():
        print(f"error: {pub_path} already exists — refusing to overwrite it. Choose a different --out path.", file=sys.stderr)
        return 1

    key = Ed25519PrivateKey.generate()
    seed_hex = key.private_bytes(Encoding.Raw, PrivateFormat.Raw, NoEncryption()).hex()
    pubkey_bytes = key.public_key().public_bytes(Encoding.Raw, PublicFormat.Raw)
    pubkey_hex = pubkey_bytes.hex()

    try:
        write_seed_file(out_path, seed_hex)
    except KeygenError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1
    finally:
        seed_hex = None  # best-effort: drop the reference promptly (see module docstring)

    try:
        write_pub_file(out_path, pubkey_hex)
    except KeygenError as exc:
        print(f"error: {exc}", file=sys.stderr)
        print(
            f"warning: {out_path} was already written before this failure — remove it manually if you want to retry with a fresh key",
            file=sys.stderr,
        )
        return 1

    print(f"[keygen] wrote curator private key seed to {out_path} (mode 0600)", file=sys.stderr)
    print(f"[keygen] wrote public key to {pub_path} (not secret)", file=sys.stderr)
    print(f"[keygen] now set CURATOR_KEY_FILE={out_path} in tools/pipeline/.env", file=sys.stderr)
    print(pubkey_hex)
    print(rust_literal(pubkey_bytes))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
