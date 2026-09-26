#!/usr/bin/env python3
"""sign_pack.py — Phase 1h Task M4a: detached ed25519 signature over a .kpack.

Signs the EXACT bytes of an already-built curated knowledge pack
(`<name>.kpack`, the SQLite file `kpack-cli` writes) with the curator
ed25519 private key, producing `<name>.kpack.sig`: a RAW 64-byte binary
detached signature, byte-for-byte what `kpack_core::sign::verify_detached`
(`verify_strict` under the hood) and `kpack_core::sign::verify_file` expect
on the Rust side. The `.sig` name is the pack's FULL file name plus `.sig`
(`foo.kpack` -> `foo.kpack.sig`, not `foo.sig`), matching
`kpack_core::manifest`'s companion-path rule. No canonicalization, no
base64/hex, no trailing newline.

This is `sign_catalog.py`'s structure applied to a pack, and it shares
that script's crypto and key-handling code by import rather than
re-implementing it — one sign path, one verify path, one CURATOR_KEY_FILE
loader with the same permission refusal. The founder runs the sign mode;
nothing in CI or in an agent session ever does.

Env-dumb (see README.md "The env-dumb contract"): the only inputs are the
CLI flags below and, ONLY for the default sign mode (not --verify-with, not
--self-test), `tools/pipeline/.env`'s `CURATOR_KEY_FILE` value. It never
reads the calling shell's exported environment.

Inputs:
    --pack <path>       The .kpack to sign (default mode) or verify (with
                         --verify-with). Required unless --self-test.
    --verify-with <hex> Verify-ONLY mode: checks `<pack>.sig` against
                         `<pack>` using the given 64-hex-char ed25519
                         PUBLIC key (the production one is
                         `kpack_core::sign::CURATOR_PUBLIC_KEY`). No private
                         key is read; `.env` is not consulted at all.
    --self-test         Throwaway-key sign/verify round trip plus the
                         RFC 8032 §7.1 TEST 1 known-answer vector (proves the
                         raw 64-byte R||S layout the Rust side parses), then
                         exits. NEVER touches --pack, CURATOR_KEY_FILE or .env.
    tools/pipeline/.env: CURATOR_KEY_FILE (sign mode only) — a file OUTSIDE
                    this repository holding the 32-byte seed as 64 hex chars.
                    Refused (not read) if unset, missing, or group/other
                    readable (see sign_catalog.check_key_file_perms).

Outputs:
    <pack>.sig — RAW 64-byte detached ed25519 signature over the EXACT bytes
    of <pack>. Written atomically (`.tmp` + rename).

Importable API:
    sign_pack_file(pack: Path, env_file: Path = ENV_FILE) -> Path
    verify_pack_file(pack: Path, pubkey_hex: str) -> bool
    sign_bytes / verify_bytes (re-exported from sign_catalog)
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
from cryptography.hazmat.primitives.serialization import Encoding, NoEncryption, PrivateFormat, PublicFormat

PIPELINE_ROOT = Path(__file__).resolve().parent
if str(PIPELINE_ROOT) not in sys.path:
    sys.path.insert(0, str(PIPELINE_ROOT))

from sign_catalog import (  # noqa: E402  (shared crypto + key handling; see module doc)
    SIG_BYTES_LEN,
    SigningError,
    load_curator_seed_hex,
    sign_bytes,
    verify_bytes,
    write_sig_atomic,
)

ENV_FILE = PIPELINE_ROOT / ".env"
PACK_SUFFIX = ".kpack"

# Sanity cap. The bundled reference pack is tens of MB; ed25519 (pure, not
# pre-hashed — what verify_strict checks) signs the whole message in memory,
# so bound what this script will ever buffer. Checked via stat() before any read.
MAX_PACK_BYTES = 512 * 1024 * 1024  # 512 MiB

# RFC 8032 §7.1, TEST 1 (empty message).
RFC8032_T1_SEED = "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60"
RFC8032_T1_PUB = "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a"
RFC8032_T1_SIG = (
    "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555"
    "fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b"
)


def sig_path_for(pack_path: Path) -> Path:
    return pack_path.with_name(pack_path.name + ".sig")


def _check_pack(pack_path: Path) -> None:
    if pack_path.suffix != PACK_SUFFIX:
        raise SigningError(f"{pack_path} does not end in {PACK_SUFFIX} — this script signs knowledge packs only")
    if not pack_path.is_file():
        raise SigningError(f"{pack_path} not found")
    size = pack_path.stat().st_size
    if size > MAX_PACK_BYTES:
        raise SigningError(f"{pack_path} is {size} bytes, over the {MAX_PACK_BYTES}-byte sanity cap — refusing to sign")


def sign_pack_file(pack_path: Path, env_file: Path = ENV_FILE) -> Path:
    """Sign mode: validate the pack (suffix, existence, size cap) BEFORE the
    key is loaded, then sign its exact bytes and write `<pack>.sig`."""
    _check_pack(pack_path)
    data = pack_path.read_bytes()
    seed_hex = load_curator_seed_hex(env_file)
    try:
        sig = sign_bytes(seed_hex, data)
    finally:
        seed_hex = None  # best-effort: drop the reference promptly
    out = sig_path_for(pack_path)
    write_sig_atomic(out, sig)
    return out


def verify_pack_file(pack_path: Path, pubkey_hex: str) -> bool:
    """--verify-with mode: `<pack>.sig` against `<pack>`'s exact bytes under a
    public key. Reads no private key and no .env. A missing pack or sig is a
    SigningError; a wrong-length sig or a bad signature is False."""
    sig_path = sig_path_for(pack_path)
    if not pack_path.is_file():
        raise SigningError(f"{pack_path} not found")
    if not sig_path.is_file():
        raise SigningError(f"{sig_path} not found")
    size = pack_path.stat().st_size
    if size > MAX_PACK_BYTES:
        raise SigningError(f"{pack_path} is {size} bytes, over the {MAX_PACK_BYTES}-byte sanity cap — refusing to read it")
    with sig_path.open("rb") as fh:
        sig = fh.read(SIG_BYTES_LEN + 1)  # capped, like kpack_core's read_sig_capped
    if len(sig) != SIG_BYTES_LEN:
        print(f"[verify] {sig_path} is not exactly {SIG_BYTES_LEN} bytes — invalid", file=sys.stderr)
        return False
    return verify_bytes(pubkey_hex, pack_path.read_bytes(), sig)


def self_test() -> bool:
    ok = True

    def check(name: str, cond: bool) -> None:
        nonlocal ok
        print(f"[self-test] {'PASS' if cond else 'FAIL'}: {name}", flush=True)
        if not cond:
            ok = False

    kat = sign_bytes(RFC8032_T1_SEED, b"")
    check("RFC 8032 §7.1 TEST 1: signature matches the published vector byte for byte", kat.hex() == RFC8032_T1_SIG)
    check("RFC 8032 §7.1 TEST 1: published signature verifies under the published key",
          verify_bytes(RFC8032_T1_PUB, b"", bytes.fromhex(RFC8032_T1_SIG)))

    key = Ed25519PrivateKey.generate()
    seed_hex = key.private_bytes(Encoding.Raw, PrivateFormat.Raw, NoEncryption()).hex()
    pub_hex = key.public_key().public_bytes(Encoding.Raw, PublicFormat.Raw).hex()
    data = b"SQLite format 3\x00" + b"self-test pack bytes -- throwaway key, never the production key" * 64
    sig = sign_bytes(seed_hex, data)
    seed_hex = None
    check("sign_bytes produces exactly 64 raw bytes", len(sig) == SIG_BYTES_LEN)
    check("valid signature verifies", verify_bytes(pub_hex, data, sig))
    flipped = bytearray(data)
    flipped[len(flipped) // 2] ^= 0x01
    check("one flipped pack byte fails", not verify_bytes(pub_hex, bytes(flipped), sig))
    check("one appended pack byte fails", not verify_bytes(pub_hex, data + b"\x00", sig))
    bad_sig = bytearray(sig)
    bad_sig[63] ^= 0x01
    check("one flipped signature byte fails", not verify_bytes(pub_hex, data, bytes(bad_sig)))
    check("truncated 63-byte signature fails cleanly", not verify_bytes(pub_hex, data, sig[:-1]))
    other = Ed25519PrivateKey.generate().public_key().public_bytes(Encoding.Raw, PublicFormat.Raw).hex()
    check("correct signature under the WRONG public key fails", not verify_bytes(other, data, sig))
    return ok


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    ap = argparse.ArgumentParser(description="Sign a .kpack with the curator ed25519 key, or verify an existing .kpack.sig.")
    ap.add_argument("--pack", type=Path, default=None, help="the .kpack to sign or (with --verify-with) verify")
    ap.add_argument("--verify-with", default=None, metavar="PUBKEY_HEX",
                    help="verify-only: check <pack>.sig against <pack> with this 64-hex-char public key (no private key read)")
    ap.add_argument("--self-test", action="store_true",
                    help="throwaway-key round trip + RFC 8032 vector, then exit (never touches CURATOR_KEY_FILE/.env)")
    return ap.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    if args.self_test:
        return 0 if self_test() else 1
    if args.pack is None:
        print("error: --pack is required (unless --self-test)", file=sys.stderr)
        return 1
    if args.verify_with is not None:
        try:
            result = verify_pack_file(args.pack, args.verify_with)
        except SigningError as exc:
            print(f"error: {exc}", file=sys.stderr)
            return 1
        print(f"[verify] {sig_path_for(args.pack)} {'verifies' if result else 'DOES NOT verify'} against the given public key")
        return 0 if result else 1
    try:
        out = sign_pack_file(args.pack, env_file=ENV_FILE)
    except SigningError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1
    print(f"[sign] wrote {out} ({SIG_BYTES_LEN} bytes)", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
