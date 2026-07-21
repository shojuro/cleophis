#!/usr/bin/env python3
"""sign_catalog.py — Task A4: detached ed25519 signature over catalog.json.

Signs the EXACT bytes of an already-assembled `catalog.json` (as written by
`build_catalog.py`) with the curator ed25519 private key, producing
`catalog.json.sig`: a RAW 64-byte binary detached signature, byte-for-byte
what `kpack_core::sign::verify_detached` (`verify_strict` under the hood)
expects on the Rust side — no canonicalization, no base64/hex encoding, no
trailing-newline games. This is the ONLY script in this pipeline that ever
reads the curator private key.

Env-dumb (see README.md "The env-dumb contract"): this script's only
inputs are its CLI flags below and, ONLY for the default sign mode (not
--verify-with, not --self-test), `tools/pipeline/.env`'s `CURATOR_KEY_FILE`
value. It never reads the calling shell's exported environment and never
assumes a working directory other than its own `tools/pipeline/` root.

Inputs:
    --catalog <path>    Path to catalog.json to sign (default mode) or to
                         verify (with --verify-with). Required unless
                         --self-test is given.
    --verify-with <hex> Verify-ONLY mode: checks the existing
                         `<catalog>.sig` against `<catalog>` using the
                         given 64-hex-char ed25519 PUBLIC key. No private
                         key file is read in this mode; `.env` is not
                         consulted at all.
    --self-test         Runs a throwaway-key sign/verify round-trip
                         self-test (mirrors the Rust B1 test at the Python
                         layer — see kpack-core/src/sign.rs's own
                         `#[cfg(test)]` module) and exits. NEVER touches
                         --catalog, CURATOR_KEY_FILE, or .env — the key it
                         uses is generated in memory, once, for this run
                         only, and is never the production key.
    tools/pipeline/.env: CURATOR_KEY_FILE (required for the default sign
                    mode; read ONLY by this script among the whole
                    pipeline — see README.md's env-dumb contract and
                    .env.example's comment on this variable). Must point to
                    a file OUTSIDE this repository containing the 32-byte
                    curator ed25519 seed as 64 hex chars, one line. Refused
                    (not read) if CURATOR_KEY_FILE is unset, if the path it
                    names doesn't exist, or if that file's mode grants
                    group or other any permission at all (checked via
                    `stat`, not assumed) — warn-and-refuse, not "read it
                    anyway".

Outputs:
    <catalog>.sig — RAW 64-byte binary detached ed25519 signature over the
    EXACT bytes of <catalog> as read from disk. Written atomically (`.tmp`
    + rename).

Key material handling: the private key seed is held only in local
variables for the minimum time needed to sign, is never printed, logged,
included in any exception message, or written to any location other than
the one file the user already chose via CURATOR_KEY_FILE. (Python cannot
guarantee the interpreter zeroes freed string/bytes memory — dropping the
reference promptly is defense in depth, not the primary control; the
primary control is simply "never expose it".)

Importable API (A6's verify_published.py imports the verify side):
    sign_bytes(seed_hex: str, data: bytes) -> bytes
    verify_bytes(pubkey_hex: str, data: bytes, sig: bytes) -> bool
"""

from __future__ import annotations

import argparse
import shlex
import stat
import sys
from pathlib import Path

from cryptography.exceptions import InvalidSignature
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey, Ed25519PublicKey
from cryptography.hazmat.primitives.serialization import Encoding, NoEncryption, PrivateFormat, PublicFormat

PIPELINE_ROOT = Path(__file__).resolve().parent
ENV_FILE = PIPELINE_ROOT / ".env"

SEED_BYTES_LEN = 32
SIG_BYTES_LEN = 64

# Sanity cap on the catalog.json this script will sign — mirrors the app's
# own `catalog_dist.rs::MAX_CATALOG_BYTES` defensive cap. A catalog larger
# than this couldn't be verified by the app anyway, so refusing here beats
# signing something the consumer would reject outright.
MAX_CATALOG_BYTES = 1024 * 1024  # 1 MiB


class SigningError(RuntimeError):
    """Any refusal in key resolution, loading, or signing/verification
    preconditions. Always caught at main()'s top level and reported as a
    clean one-line error, never a raw traceback — and never one that
    includes key material."""


# ---------------------------------------------------------------------
# Core sign/verify primitives (importable; A6 imports verify_bytes).
# ---------------------------------------------------------------------


def decode_seed_hex(seed_hex: str) -> bytes:
    """Decode a 64-hex-char ed25519 seed to its raw 32 bytes. Raises
    SigningError on any malformed input — the error message never echoes
    the input itself, only its (safe to log) length/shape."""
    seed_hex = seed_hex.strip()
    try:
        seed = bytes.fromhex(seed_hex)
    except ValueError as exc:
        raise SigningError(f"curator key file content is not valid hex: {exc}") from exc
    if len(seed) != SEED_BYTES_LEN:
        raise SigningError(
            f"curator key file content decodes to {len(seed)} bytes, expected exactly "
            f"{SEED_BYTES_LEN} (64 hex chars)"
        )
    return seed


def sign_bytes(seed_hex: str, data: bytes) -> bytes:
    """Detached ed25519 signature over `data`, given a 64-hex-char seed.

    Returns exactly 64 raw bytes (RFC 8032) — the same format
    `ed25519-dalek`'s `verify_strict` (the Rust consumer) expects.
    Raises SigningError if `seed_hex` isn't a well-formed 32-byte hex seed.
    """
    seed = decode_seed_hex(seed_hex)
    key = Ed25519PrivateKey.from_private_bytes(seed)
    sig = key.sign(data)
    assert len(sig) == SIG_BYTES_LEN, f"internal error: cryptography produced a {len(sig)}-byte ed25519 signature"
    return sig


def verify_bytes(pubkey_hex: str, data: bytes, sig: bytes) -> bool:
    """True iff `sig` is a valid detached ed25519 signature over `data`
    under the public key `pubkey_hex` (64 hex chars).

    Any malformed input (wrong-length pubkey hex, non-hex pubkey,
    wrong-length signature) or an actual verification failure both return
    False — this is a plain boolean predicate, it never raises for "the
    signature doesn't verify" so callers (e.g. A6) can use it directly in
    an `if`.
    """
    try:
        pubkey_bytes = bytes.fromhex(pubkey_hex.strip())
    except ValueError:
        return False
    if len(pubkey_bytes) != SEED_BYTES_LEN:
        return False
    if len(sig) != SIG_BYTES_LEN:
        return False
    try:
        public_key = Ed25519PublicKey.from_public_bytes(pubkey_bytes)
        public_key.verify(sig, data)
        return True
    except InvalidSignature:
        return False


# ---------------------------------------------------------------------
# CURATOR_KEY_FILE resolution (.env) — sign mode only.
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


def resolve_curator_key_path(env_file: Path) -> Path:
    values = load_env_file(env_file)
    raw = values.get("CURATOR_KEY_FILE") or None
    if not raw:
        raise SigningError(
            f"CURATOR_KEY_FILE is not set in {env_file} — see .env.example. It must point to a file "
            "OUTSIDE this repo containing the curator ed25519 seed (generate one with "
            "generate_curator_key.py)."
        )
    return Path(raw).expanduser()


def check_key_file_perms(path: Path) -> None:
    if not path.is_file():
        raise SigningError(f"CURATOR_KEY_FILE points at {path}, which does not exist (or is not a regular file)")
    mode = stat.S_IMODE(path.stat().st_mode)
    if mode & 0o077:
        raise SigningError(
            f"{path} is readable by group or other (mode {oct(mode)}) — refusing to read a curator private "
            f"key from a file with loose permissions. Run: chmod 600 {shlex.quote(str(path))}"
        )


def load_curator_seed_hex(env_file: Path) -> str:
    """Resolves CURATOR_KEY_FILE from .env, checks its permissions
    (warn-and-refuse if group/other-readable), and returns its content as
    a hex string — validated to decode as a 32-byte seed, but the raw
    content itself is never logged anywhere on this path."""
    path = resolve_curator_key_path(env_file)
    check_key_file_perms(path)
    content = path.read_text().strip()
    decode_seed_hex(content)  # validates shape; raises SigningError with no key material in the message
    return content


# ---------------------------------------------------------------------
# .sig file I/O.
# ---------------------------------------------------------------------


def sig_path_for(catalog_path: Path) -> Path:
    return catalog_path.with_name(catalog_path.name + ".sig")


def write_sig_atomic(sig_path: Path, sig: bytes) -> None:
    if len(sig) != SIG_BYTES_LEN:
        raise SigningError(f"internal error: refusing to write a {len(sig)}-byte signature (expected exactly {SIG_BYTES_LEN})")
    sig_path.parent.mkdir(parents=True, exist_ok=True)
    tmp = sig_path.with_name(sig_path.name + ".tmp")
    tmp.write_bytes(sig)
    tmp.replace(sig_path)  # atomic rename on POSIX, same filesystem


def verify_catalog_file(catalog_path: Path, pubkey_hex: str) -> bool:
    """--verify-with mode: verifies `<catalog_path>.sig` against
    `catalog_path`'s exact bytes, given only a public key. No private key
    or .env involvement at all."""
    sig_path = sig_path_for(catalog_path)
    if not catalog_path.is_file():
        raise SigningError(f"{catalog_path} not found")
    if not sig_path.is_file():
        raise SigningError(f"{sig_path} not found")
    data = catalog_path.read_bytes()
    sig = sig_path.read_bytes()
    if len(sig) != SIG_BYTES_LEN:
        print(f"[verify] {sig_path} is {len(sig)} bytes, expected exactly {SIG_BYTES_LEN} — invalid", file=sys.stderr)
        return False
    return verify_bytes(pubkey_hex, data, sig)


# ---------------------------------------------------------------------
# --self-test: throwaway-key round trip (never CURATOR_KEY_FILE/.env).
# ---------------------------------------------------------------------


def self_test() -> bool:
    """Generates a throwaway ed25519 keypair IN MEMORY (never written
    anywhere, never the production key) and exercises: sign -> verify OK;
    flip one byte of the signed data -> fail; flip one byte of the
    signature -> fail; truncated signature -> fail cleanly; oversized
    signature -> fail cleanly; wrong public key -> fail. Mirrors
    kpack-core/src/sign.rs's own `#[cfg(test)]` coverage at the Python
    layer, per the task brief. Returns True iff every check passes.
    """
    ok = True

    def check(name: str, cond: bool) -> None:
        nonlocal ok
        print(f"[self-test] {'PASS' if cond else 'FAIL'}: {name}", flush=True)
        if not cond:
            ok = False

    key = Ed25519PrivateKey.generate()
    seed_hex = key.private_bytes(Encoding.Raw, PrivateFormat.Raw, NoEncryption()).hex()
    pubkey_hex = key.public_key().public_bytes(Encoding.Raw, PublicFormat.Raw).hex()

    data = b"self-test catalog bytes -- throwaway key, never the production key"
    sig = sign_bytes(seed_hex, data)
    seed_hex = None  # done with it; drop the reference promptly

    check("sign_bytes produces exactly 64 bytes", len(sig) == SIG_BYTES_LEN)
    check("verify_bytes: valid signature verifies", verify_bytes(pubkey_hex, data, sig) is True)

    tampered_data = bytearray(data)
    tampered_data[0] ^= 0x01
    check("verify_bytes: one flipped data byte fails", verify_bytes(pubkey_hex, bytes(tampered_data), sig) is False)

    tampered_sig = bytearray(sig)
    tampered_sig[0] ^= 0x01
    check("verify_bytes: one flipped signature byte fails", verify_bytes(pubkey_hex, data, bytes(tampered_sig)) is False)

    check("verify_bytes: truncated (63-byte) signature fails cleanly", verify_bytes(pubkey_hex, data, sig[:-1]) is False)
    check("verify_bytes: oversized (65-byte) signature fails cleanly", verify_bytes(pubkey_hex, data, sig + b"\x00") is False)

    wrong_key = Ed25519PrivateKey.generate()
    wrong_pubkey_hex = wrong_key.public_key().public_bytes(Encoding.Raw, PublicFormat.Raw).hex()
    check("verify_bytes: correct signature under the WRONG public key fails", verify_bytes(wrong_pubkey_hex, data, sig) is False)

    return ok


# ---------------------------------------------------------------------
# CLI.
# ---------------------------------------------------------------------


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Sign catalog.json with the curator ed25519 private key, or verify an existing signature.",
    )
    parser.add_argument("--catalog", type=Path, default=None, help="path to catalog.json to sign or (with --verify-with) verify")
    parser.add_argument(
        "--verify-with",
        default=None,
        metavar="PUBKEY_HEX",
        help="verify-only mode: check <catalog>.sig against <catalog> using this 64-hex-char public key (no private key read)",
    )
    parser.add_argument(
        "--self-test",
        action="store_true",
        help="run the throwaway-key sign/verify self-test and exit (never touches CURATOR_KEY_FILE/.env)",
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)

    if args.self_test:
        return 0 if self_test() else 1

    if args.catalog is None:
        print("error: --catalog is required (unless --self-test)", file=sys.stderr)
        return 1

    if args.verify_with is not None:
        try:
            result = verify_catalog_file(args.catalog, args.verify_with)
        except SigningError as exc:
            print(f"error: {exc}", file=sys.stderr)
            return 1
        print(f"[verify] {sig_path_for(args.catalog)} {'verifies' if result else 'DOES NOT verify'} against the given public key")
        return 0 if result else 1

    if not args.catalog.is_file():
        print(f"error: {args.catalog} not found", file=sys.stderr)
        return 1

    # Check the size via stat() BEFORE reading — an oversized file must
    # never be read into memory just to be rejected; that would defeat the
    # whole point of the cap (bounding how much this script ever buffers).
    catalog_size = args.catalog.stat().st_size
    if catalog_size > MAX_CATALOG_BYTES:
        print(
            f"error: {args.catalog} is {catalog_size} bytes, exceeding the {MAX_CATALOG_BYTES}-byte "
            "sanity cap (matches the app's own fetch cap) — refusing to sign",
            file=sys.stderr,
        )
        return 1

    catalog_bytes = args.catalog.read_bytes()

    try:
        seed_hex = load_curator_seed_hex(ENV_FILE)
    except SigningError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1

    try:
        sig = sign_bytes(seed_hex, catalog_bytes)
    finally:
        seed_hex = None  # best-effort: drop the reference promptly (see module docstring)

    sig_path = sig_path_for(args.catalog)
    write_sig_atomic(sig_path, sig)
    print(f"[sign] wrote {sig_path} ({len(sig)} bytes)", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
