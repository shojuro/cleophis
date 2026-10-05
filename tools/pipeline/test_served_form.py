#!/usr/bin/env python3
"""Phase 1g Task M3 — quant-driven bases, name-driven adapters, carry-forward.

Run from anywhere (no network, no credentials, no .env):

    python3 -m pytest -q tools/pipeline/test_served_form.py

The fixture `fixtures/catalog-v10.json` (+ `.sig`) is the LIVE signed dist
catalog v10, fetched verbatim with one public GET on 2026-09-26. Its
signature verifies under the production curator public key pinned in
`crates/kpack-core/src/sign.rs` (`CURATOR_PUBLIC_KEY`), repeated below.
"""

from __future__ import annotations

import json
import sys
import tempfile
import unittest
from pathlib import Path

PIPELINE = Path(__file__).resolve().parent
sys.path.insert(0, str(PIPELINE))

import build_catalog as bc  # noqa: E402
import publish as pb  # noqa: E402

FIXTURE = PIPELINE / "fixtures" / "catalog-v10.json"
PROD_PUBKEY = "158cb99e9756e2e4d01d88b7ecfeb99a76547821ffe9f9f1985316c5451cd0c0"

SERVED_BASE_SHA = "2588912fe87f55b8381b9fc8faacd0e905c9eb2db641a468be693f45ad6fc87a"
SERVED_BASE_SIZE = 1_673_006_944
TRIAGE_V3_SHA = "5304e464cd485e8a7d8eb75083363e3cc4de0f665e2c785dbd1a1f7e93d13a20"


def live_artifacts() -> list[dict]:
    return json.loads(FIXTURE.read_text())["artifacts"]


def by_path(path: str) -> dict:
    return next(a for a in live_artifacts() if a["path"] == path)


def synth(kind: str, base_model: str, name: str, *, from_path: str, **extra) -> dict:
    """A synthetic manifest carrying the live entry's sha/size/license, so a
    re-derived entry can be compared field-for-field with the live one."""
    live = by_path(from_path)
    m = {"kind": kind, "base_model": base_model, "name": name,
         "sha256": live["sha256"], "size": live["size"], "license": live["license"]}
    m.update(extra)
    return m


# Hand-written, one per live entry, in live order. Deliberately NOT derived
# from the path by build_catalog's own parser (that would be tautological).
LIVE_SYNTHETIC_MANIFESTS = [
    synth("base", "Qwen3-4B", "Qwen3-4B-Instruct-Q4_K_M.gguf", quant="Q4_K_M",
          from_path="models/Qwen3-4B/v1/Qwen3-4B-Instruct-Q4_K_M.gguf"),
    # back-compat: no adapter_name/version -> behavioral + v1
    synth("adapter", "Qwen3-4B", "behavioral-v1-Qwen3-4B.gguf",
          from_path="adapters/behavioral/v1/Qwen3-4B/behavioral-v1-Qwen3-4B.gguf"),
    synth("base", "Llama-3.2-1B", "Llama-3.2-1B-Instruct-Q4_K_M.gguf", quant="Q4_K_M",
          from_path="models/Llama-3.2-1B/v1/Llama-3.2-1B-Instruct-Q4_K_M.gguf"),
    # explicit behavioral + v1 must give the same thing
    synth("adapter", "Llama-3.2-1B", "behavioral-v1-Llama-3.2-1B.gguf", adapter_name="behavioral", version="v1",
          from_path="adapters/behavioral/v1/Llama-3.2-1B/behavioral-v1-Llama-3.2-1B.gguf"),
    synth("base", "Qwen3-8B", "Qwen3-8B-Instruct-Q4_K_M.gguf", quant="Q4_K_M",
          from_path="models/Qwen3-8B/v1/Qwen3-8B-Instruct-Q4_K_M.gguf"),
    synth("adapter", "Qwen3-8B", "behavioral-v1-Qwen3-8B.gguf",
          from_path="adapters/behavioral/v1/Qwen3-8B/behavioral-v1-Qwen3-8B.gguf"),
    synth("base", "Qwen3-1.7B", "Qwen3-1.7B-Instruct-Q4_K_M.gguf", quant="Q4_K_M",
          from_path="models/Qwen3-1.7B/v1/Qwen3-1.7B-Instruct-Q4_K_M.gguf"),
    synth("adapter", "Qwen3-1.7B", "behavioral-v1-Qwen3-1.7B.gguf",
          from_path="adapters/behavioral/v1/Qwen3-1.7B/behavioral-v1-Qwen3-1.7B.gguf"),
    # contract adapter at a dotted version, via the new `version` field
    synth("contract-adapter", "Qwen3-4B", "contract-v3.1-Qwen3-4B.gguf", adapter_name="contract", version="v3.1",
          from_path="adapters/contract/v3.1/Qwen3-4B/contract-v3.1-Qwen3-4B.gguf"),
    synth("adapter", "Qwen3-4B", "voice-adult-v1-Qwen3-4B.gguf", adapter_name="voice-adult", version="v1",
          from_path="adapters/voice-adult/v1/Qwen3-4B/voice-adult-v1-Qwen3-4B.gguf"),
    synth("adapter", "Llama-3.2-1B", "tutor-v1.3-Llama-3.2-1B.gguf", adapter_name="tutor", version="v1.3",
          from_path="adapters/tutor/v1.3/Llama-3.2-1B/tutor-v1.3-Llama-3.2-1B.gguf"),
]

SERVED_BASE_MANIFEST = {
    "kind": "base", "base_model": "Qwen3-1.7B", "name": "Qwen3-1.7B-Instruct-Q6_K.gguf", "quant": "Q6_K",
    "sha256": SERVED_BASE_SHA, "size": SERVED_BASE_SIZE, "license": "Apache-2.0",
}
TRIAGE_V3_MANIFEST = {
    "kind": "adapter", "base_model": "Qwen3-1.7B", "name": "triage-v3-Qwen3-1.7B.gguf",
    "adapter_name": "triage", "version": "v3",
    "sha256": TRIAGE_V3_SHA, "size": 34_892_608, "license": "Apache-2.0",
}


def to_artifact(manifest: dict) -> dict:
    return bc.manifest_to_artifact(manifest, Path("synthetic/manifest.json"), {})


def refuses(fn) -> bool:
    try:
        fn()
    except bc.CatalogAssemblyError:
        return True
    return False


class ElevenLivePathsRegression(unittest.TestCase):
    def test_fixture_is_the_signed_live_v10(self):
        from sign_catalog import verify_bytes
        data = FIXTURE.read_bytes()
        sig = FIXTURE.with_name(FIXTURE.name + ".sig").read_bytes()
        self.assertTrue(verify_bytes(PROD_PUBKEY, data, sig))
        self.assertEqual(json.loads(data)["catalog_version"], 10)
        self.assertEqual(len(live_artifacts()), 11)

    def test_every_live_entry_rederives_field_for_field(self):
        derived = [to_artifact(m) for m in LIVE_SYNTHETIC_MANIFESTS]
        self.assertEqual(derived, live_artifacts())

    def test_behavioral_default_equals_explicit(self):
        base = {k: v for k, v in LIVE_SYNTHETIC_MANIFESTS[1].items()}
        explicit = dict(base, adapter_name="behavioral", version="v1")
        self.assertEqual(to_artifact(base), to_artifact(explicit))
        self.assertEqual(to_artifact(base)["path"], "adapters/behavioral/v1/Qwen3-4B/behavioral-v1-Qwen3-4B.gguf")

    def test_legacy_contract_dist_version_still_accepted(self):
        m = dict(LIVE_SYNTHETIC_MANIFESTS[8])
        del m["adapter_name"], m["version"]
        m["dist_version"] = "v3.1"
        self.assertEqual(to_artifact(m), by_path("adapters/contract/v3.1/Qwen3-4B/contract-v3.1-Qwen3-4B.gguf"))


class ServedFormDerivations(unittest.TestCase):
    def test_known_base_models_has_qwen3_1_7b(self):
        self.assertIn("Qwen3-1.7B", bc.KNOWN_BASE_MODELS)

    def test_served_q6k_base(self):
        self.assertEqual(to_artifact(SERVED_BASE_MANIFEST), {
            "path": "models/Qwen3-1.7B/v1/Qwen3-1.7B-Instruct-Q6_K.gguf",
            "sha256": SERVED_BASE_SHA, "size": SERVED_BASE_SIZE, "kind": "base",
            "base_model": "Qwen3-1.7B", "version": "v1", "license": "Apache-2.0",
        })

    def test_triage_v3_adapter(self):
        a = to_artifact(TRIAGE_V3_MANIFEST)
        self.assertEqual(a["path"], "adapters/triage/v3/Qwen3-1.7B/triage-v3-Qwen3-1.7B.gguf")
        self.assertEqual(a["kind"], "adapter")
        self.assertEqual(a["sha256"], TRIAGE_V3_SHA)
        # kind "adapter" entries carry the fixed wire "v1" (as the live tutor-v1.3 entry does)
        self.assertEqual(a["version"], "v1")

    def test_refusals(self):
        cases = {
            "basename not the derived one": dict(TRIAGE_V3_MANIFEST, name="triage-armb-v3-Qwen3-1.7B.gguf"),
            "base name vs quant mismatch": dict(SERVED_BASE_MANIFEST, quant="Q4_K_M"),
            "base without quant": {k: v for k, v in SERVED_BASE_MANIFEST.items() if k != "quant"},
            "hostile quant": dict(SERVED_BASE_MANIFEST, quant="../Q6_K", name="Qwen3-1.7B-Instruct-../Q6_K.gguf"),
            "path-traversal adapter_name": dict(TRIAGE_V3_MANIFEST, adapter_name="../triage", name="../triage-v3-Qwen3-1.7B.gguf"),
            "upper-case adapter_name": dict(TRIAGE_V3_MANIFEST, adapter_name="Triage", name="Triage-v3-Qwen3-1.7B.gguf"),
            "version without v": dict(TRIAGE_V3_MANIFEST, version="3", name="triage-3-Qwen3-1.7B.gguf"),
            "version with slash": dict(TRIAGE_V3_MANIFEST, version="v3/x", name="triage-v3/x-Qwen3-1.7B.gguf"),
            "kind adapter named contract": dict(TRIAGE_V3_MANIFEST, adapter_name="contract", name="contract-v3-Qwen3-1.7B.gguf"),
            "contract-adapter with another name": dict(LIVE_SYNTHETIC_MANIFESTS[8], adapter_name="tutor", name="tutor-v3.1-Qwen3-4B.gguf"),
            "version and dist_version disagree": dict(LIVE_SYNTHETIC_MANIFESTS[8], dist_version="v2"),
            "unknown base model": dict(TRIAGE_V3_MANIFEST, base_model="Qwen3-0.6B", name="triage-v3-Qwen3-0.6B.gguf"),
        }
        for label, m in cases.items():
            with self.subTest(label):
                self.assertTrue(refuses(lambda m=m: to_artifact(m)), label)


class CarryForward(unittest.TestCase):
    def test_loads_verified_live_entries_verbatim(self):
        carried = bc.load_carry_forward(FIXTURE, PROD_PUBKEY)
        self.assertEqual(carried.catalog_version, 10)
        self.assertEqual(carried.artifacts, live_artifacts())

    def test_wrong_key_or_tampered_bytes_refused(self):
        self.assertTrue(refuses(lambda: bc.load_carry_forward(FIXTURE, "00" * 32)))
        with tempfile.TemporaryDirectory() as d:
            p = Path(d) / "catalog.json"
            p.write_bytes(FIXTURE.read_bytes().replace(b'"size": 66095392', b'"size": 66095393'))
            p.with_name("catalog.json.sig").write_bytes(FIXTURE.with_name(FIXTURE.name + ".sig").read_bytes())
            self.assertTrue(refuses(lambda: bc.load_carry_forward(p, PROD_PUBKEY)))
            (Path(d) / "nosig.json").write_bytes(FIXTURE.read_bytes())
            self.assertTrue(refuses(lambda: bc.load_carry_forward(Path(d) / "nosig.json", PROD_PUBKEY)))

    def test_underivable_carried_entry_refused(self):
        entries = live_artifacts()
        entries[0] = dict(entries[0], path="models/Qwen3-4B/v2/Qwen3-4B-Instruct-Q4_K_M.gguf")
        self.assertTrue(refuses(lambda: bc.check_carried_entries(entries, Path("x"))))
        entries = live_artifacts()
        entries[1] = dict(entries[1], extra="field")
        self.assertTrue(refuses(lambda: bc.check_carried_entries(entries, Path("x"))))

    def _write(self, d: Path, name: str, m: dict) -> Path:
        p = d / name
        p.write_text(json.dumps(m))
        return p

    def test_build_v11_with_carry_forward(self):
        carried = bc.load_carry_forward(FIXTURE, PROD_PUBKEY)
        with tempfile.TemporaryDirectory() as d:
            d = Path(d)
            paths = [self._write(d, "b.json", SERVED_BASE_MANIFEST), self._write(d, "a.json", TRIAGE_V3_MANIFEST)]
            cat = bc.build_catalog(paths, 11, {}, carried=carried.artifacts)
        self.assertEqual(cat["catalog_version"], 11)
        self.assertEqual(len(cat["artifacts"]), 13)
        self.assertEqual(cat["artifacts"][:11], live_artifacts())
        self.assertEqual([a["path"] for a in cat["artifacts"][11:]], [
            "models/Qwen3-1.7B/v1/Qwen3-1.7B-Instruct-Q6_K.gguf",
            "adapters/triage/v3/Qwen3-1.7B/triage-v3-Qwen3-1.7B.gguf",
        ])

    def test_duplicate_path_refused(self):
        carried = bc.load_carry_forward(FIXTURE, PROD_PUBKEY)
        with tempfile.TemporaryDirectory() as d:
            p = self._write(Path(d), "dup.json", LIVE_SYNTHETIC_MANIFESTS[0])
            self.assertTrue(refuses(lambda: bc.build_catalog([p], 11, {}, carried=carried.artifacts)))

    def test_version_must_be_newer_than_carried(self):
        self.assertTrue(refuses(lambda: bc.check_version_after_carried(10, 10)))
        self.assertTrue(refuses(lambda: bc.check_version_after_carried(9, 10)))
        bc.check_version_after_carried(11, 10)

    def test_stale_carry_forward_vs_published_refused(self):
        self.assertTrue(refuses(lambda: bc.check_carried_is_published(carried_version=10, published_version=11)))
        bc.check_carried_is_published(carried_version=10, published_version=10)


class PublishLocalResolution(unittest.TestCase):
    OUT = Path("/nonexistent/out")

    def test_base_subdir_is_the_quant(self):
        a = to_artifact(SERVED_BASE_MANIFEST)
        self.assertEqual(pb.local_artifact_path(self.OUT, a), self.OUT / "Qwen3-1.7B/Q6_K/Qwen3-1.7B-Instruct-Q6_K.gguf")
        q4 = by_path("models/Qwen3-4B/v1/Qwen3-4B-Instruct-Q4_K_M.gguf")
        self.assertEqual(pb.local_artifact_path(self.OUT, q4), self.OUT / "Qwen3-4B/Q4_K_M/Qwen3-4B-Instruct-Q4_K_M.gguf")

    def test_adapters_unchanged(self):
        a = to_artifact(TRIAGE_V3_MANIFEST)
        self.assertEqual(pb.local_artifact_path(self.OUT, a), self.OUT / "Qwen3-1.7B/adapter/triage-v3-Qwen3-1.7B.gguf")
        c = by_path("adapters/contract/v3.1/Qwen3-4B/contract-v3.1-Qwen3-4B.gguf")
        self.assertEqual(pb.local_artifact_path(self.OUT, c), self.OUT / "Qwen3-4B/adapter/contract-v3.1-Qwen3-4B.gguf")

    def test_base_without_quant_in_basename_refused(self):
        a = dict(to_artifact(SERVED_BASE_MANIFEST), path="models/Qwen3-1.7B/v1/Qwen3-1.7B.gguf")
        with self.assertRaises(pb.PublishError):
            pb.local_artifact_path(self.OUT, a)
        a = dict(a, path="models/Qwen3-1.7B/v1/Qwen3-1.7B-Instruct-..gguf")
        with self.assertRaises(pb.PublishError):
            pb.local_artifact_path(self.OUT, a)


class PublishCarriedForward(unittest.TestCase):
    def test_carried_set_is_exact_entry_match(self):
        carried = pb.load_carried_forward_entries(FIXTURE)
        live = live_artifacts()
        self.assertTrue(pb.is_carried(live[0], carried))
        self.assertFalse(pb.is_carried(dict(live[0], sha256="0" * 64), carried))
        self.assertFalse(pb.is_carried(to_artifact(SERVED_BASE_MANIFEST), carried))

    def test_remote_only_requires_matching_remote(self):
        art = live_artifacts()[1]
        client = pb._FakeS3Client()
        with self.assertRaises(pb.PublishError):
            pb.confirm_carried_artifact(client, "cleophis-dist", art)
        client._seed("cleophis-dist", art["path"], b"x", sha256_hex="f" * 64)
        with self.assertRaises(pb.ImmutabilityViolation):
            pb.confirm_carried_artifact(client, "cleophis-dist", art)
        client2 = pb._FakeS3Client()
        client2._seed("cleophis-dist", art["path"], b"x", sha256_hex=art["sha256"])
        self.assertEqual(pb.confirm_carried_artifact(client2, "cleophis-dist", art), "present")
        self.assertFalse(any(op in ("upload_file", "put_bytes") for op, _, _ in client2.calls))


if __name__ == "__main__":
    unittest.main()
