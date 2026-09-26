#!/usr/bin/env python3
"""Phase 1h Task M4a — sign_pack.py: detached ed25519 over a .kpack's exact bytes.

Run from anywhere (no network, no credentials, never reads tools/pipeline/.env):

    python3 -m pytest -q tools/pipeline/test_sign_pack.py

Every key here is generated in memory for the test. The production key is
never touched: sign-mode tests pass a TEMPORARY env file explicitly, and
the CLI tests only exercise --self-test and --verify-with, which read no key.
"""

from __future__ import annotations

import io
import os
import sys
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path
from unittest import mock

PIPELINE = Path(__file__).resolve().parent
sys.path.insert(0, str(PIPELINE))

from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey  # noqa: E402
from cryptography.hazmat.primitives.serialization import Encoding, NoEncryption, PrivateFormat, PublicFormat  # noqa: E402

import sign_pack as sp  # noqa: E402


def keypair() -> tuple[str, str]:
    k = Ed25519PrivateKey.generate()
    return (
        k.private_bytes(Encoding.Raw, PrivateFormat.Raw, NoEncryption()).hex(),
        k.public_key().public_bytes(Encoding.Raw, PublicFormat.Raw).hex(),
    )


class SigPath(unittest.TestCase):
    def test_sig_sits_beside_the_pack_with_the_full_name(self):
        # kpack_core::manifest: `foo.kpack` gets `foo.kpack.sig`, not `foo.sig`
        self.assertEqual(sp.sig_path_for(Path("/x/reference.kpack")), Path("/x/reference.kpack.sig"))


class SignAndVerify(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.dir = Path(self.tmp.name)
        self.pack = self.dir / "reference.kpack"
        self.pack.write_bytes(b"SQLite format 3\x00" + os.urandom(4096))
        self.seed, self.pub = keypair()

    def tearDown(self):
        self.tmp.cleanup()

    def _env_with_key(self, mode=0o600) -> Path:
        keyf = self.dir / "curator.key"
        keyf.write_text(self.seed + "\n")
        keyf.chmod(mode)
        env = self.dir / "test.env"
        env.write_text(f"CURATOR_KEY_FILE={keyf}\n")
        return env

    def test_sign_writes_raw_64_bytes_over_the_exact_pack_bytes(self):
        sig_path = sp.sign_pack_file(self.pack, env_file=self._env_with_key())
        sig = sig_path.read_bytes()
        self.assertEqual(len(sig), 64)
        self.assertTrue(sp.verify_bytes(self.pub, self.pack.read_bytes(), sig))
        self.assertTrue(sp.verify_pack_file(self.pack, self.pub))

    def test_any_change_to_the_pack_breaks_the_signature(self):
        sp.sign_pack_file(self.pack, env_file=self._env_with_key())
        data = bytearray(self.pack.read_bytes())
        data[-1] ^= 0x01
        self.pack.write_bytes(bytes(data))
        self.assertFalse(sp.verify_pack_file(self.pack, self.pub))

    def test_wrong_public_key_fails(self):
        sp.sign_pack_file(self.pack, env_file=self._env_with_key())
        _s, other_pub = keypair()
        self.assertFalse(sp.verify_pack_file(self.pack, other_pub))

    def test_loose_key_permissions_are_refused_and_no_sig_written(self):
        with self.assertRaises(sp.SigningError):
            sp.sign_pack_file(self.pack, env_file=self._env_with_key(mode=0o644))
        self.assertFalse(sp.sig_path_for(self.pack).exists())

    def test_missing_curator_key_file_setting_is_refused(self):
        env = self.dir / "empty.env"
        env.write_text("# nothing\n")
        with self.assertRaises(sp.SigningError):
            sp.sign_pack_file(self.pack, env_file=env)

    def test_oversized_pack_is_refused_before_reading(self):
        with mock.patch.object(sp, "MAX_PACK_BYTES", 10):
            with self.assertRaises(sp.SigningError):
                sp.sign_pack_file(self.pack, env_file=self._env_with_key())

    def test_non_kpack_suffix_is_refused(self):
        other = self.dir / "catalog.json"
        other.write_bytes(b"{}")
        with self.assertRaises(sp.SigningError):
            sp.sign_pack_file(other, env_file=self._env_with_key())

    def test_truncated_or_oversized_sig_file_fails_cleanly(self):
        sp.sign_pack_file(self.pack, env_file=self._env_with_key())
        sigp = sp.sig_path_for(self.pack)
        good = sigp.read_bytes()
        sigp.write_bytes(good[:-1])
        self.assertFalse(sp.verify_pack_file(self.pack, self.pub))
        sigp.write_bytes(good + b"\x00")
        self.assertFalse(sp.verify_pack_file(self.pack, self.pub))


class Cli(unittest.TestCase):
    def test_self_test_passes_and_touches_no_key(self):
        with mock.patch.object(sp, "load_curator_seed_hex", side_effect=AssertionError("key read")):
            out = io.StringIO()
            with redirect_stdout(out):
                rc = sp.main(["--self-test"])
        self.assertEqual(rc, 0, out.getvalue())
        self.assertIn("RFC 8032", out.getvalue())
        self.assertNotIn("FAIL", out.getvalue())

    def test_verify_with_reads_no_key_and_reports(self):
        seed, pub = keypair()
        with tempfile.TemporaryDirectory() as d:
            pack = Path(d) / "p.kpack"
            pack.write_bytes(b"pack bytes")
            sp.sig_path_for(pack).write_bytes(sp.sign_bytes(seed, b"pack bytes"))
            with mock.patch.object(sp, "load_curator_seed_hex", side_effect=AssertionError("key read")):
                with redirect_stdout(io.StringIO()), redirect_stderr(io.StringIO()):
                    self.assertEqual(sp.main(["--pack", str(pack), "--verify-with", pub]), 0)
                    _s, wrong = keypair()
                    self.assertEqual(sp.main(["--pack", str(pack), "--verify-with", wrong]), 1)

    def test_pack_is_required_outside_self_test(self):
        with redirect_stderr(io.StringIO()):
            self.assertEqual(sp.main([]), 1)

    def test_sign_mode_reads_only_the_pipeline_env_file(self):
        # The CLI's sign mode must resolve the key through tools/pipeline/.env and
        # nothing else. We intercept the loader instead of reading the real file.
        with tempfile.TemporaryDirectory() as d:
            pack = Path(d) / "p.kpack"
            pack.write_bytes(b"x")
            seen = {}

            def fake_loader(env_file):
                seen["env"] = env_file
                raise sp.SigningError("stop here")

            with mock.patch.object(sp, "load_curator_seed_hex", side_effect=fake_loader):
                with redirect_stderr(io.StringIO()):
                    self.assertEqual(sp.main(["--pack", str(pack)]), 1)
            self.assertEqual(seen["env"], PIPELINE / ".env")
            self.assertFalse(sp.sig_path_for(pack).exists())


if __name__ == "__main__":
    unittest.main()
