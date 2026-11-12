#!/usr/bin/env python3
"""P00 — `download_peft_dir_from_s3` must never write outside dest_dir.

Run from anywhere (no network, no credentials, no .env; boto3 is faked):

    python3 -m pytest -q tools/pipeline/test_build_adapter_s3.py
"""

from __future__ import annotations

import sys
import tempfile
import types
import unittest
from pathlib import Path
from unittest import mock

PIPELINE = Path(__file__).resolve().parent
sys.path.insert(0, str(PIPELINE))

import build_adapter as ba  # noqa: E402

PREFIX = "adapters/x/"


class _FakeClient:
    def __init__(self, keys: list[str]) -> None:
        self._keys = keys
        self.downloaded: list[str] = []

    def get_paginator(self, _name: str):
        keys = self._keys

        class _Pag:
            def paginate(self, **_kw):
                return [{"Contents": [{"Key": k, "Size": 2, "ETag": '"x-2"'} for k in keys]}]

        return _Pag()

    def download_file(self, _bucket: str, key: str, dest: str) -> None:
        self.downloaded.append(key)
        Path(dest).write_bytes(b"{}")


def _run(keys: list[str], dest: Path) -> _FakeClient:
    client = _FakeClient(keys)
    fake_boto3 = types.SimpleNamespace(client=lambda *a, **k: client)
    with mock.patch.dict(sys.modules, {"boto3": fake_boto3}):
        ba.download_peft_dir_from_s3("b", PREFIX, dest, "https://e", "id", "k", max_attempts=1)
    return client


class DownloadContainmentTest(unittest.TestCase):
    def test_normal_file_accepted(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            dest = Path(tmp) / "dest"
            client = _run([PREFIX + "adapter_config.json"], dest)
            self.assertEqual(client.downloaded, [PREFIX + "adapter_config.json"])
            self.assertTrue((dest / "adapter_config.json").is_file())

    def test_nested_file_accepted(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            dest = Path(tmp) / "dest"
            _run([PREFIX + "sub/adapter_model.safetensors"], dest)
            self.assertTrue((dest / "sub" / "adapter_model.safetensors").is_file())

    def test_hostile_keys_refused(self) -> None:
        for rel in ["../../x", "/abs", "a/../b", "a//b", "a\\b", "./a", "a/./b"]:
            with self.subTest(rel=rel), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                dest = root / "a" / "b" / "dest"
                key = PREFIX + rel
                with self.assertRaises(RuntimeError) as cm:
                    _run([key], dest)
                self.assertIn(repr(key), str(cm.exception))
                written = [p for p in root.rglob("*") if p.is_file()]
                self.assertEqual(written, [], f"files written: {written}")


if __name__ == "__main__":
    unittest.main()
