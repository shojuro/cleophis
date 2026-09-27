#!/usr/bin/env python3
"""Phase 1h Task M4a — clusters.py: entity clusters, the seeded split, the held-out sets.

Synthetic mini-corpus + synthetic bank/partition for the unit tests; the
last class checks the COMMITTED reference-clusters.json against the
committed corpus and (when present on this machine) the triage repo's
frozen bank and partition. Run:

    python3 -m pytest -q tools/reference/tests/test_clusters.py
"""

from __future__ import annotations

import json
import sys
import tempfile
import unittest
from pathlib import Path

REFERENCE = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REFERENCE))

import clusters as cl  # noqa: E402
import fetch_nhs as fn  # noqa: E402

TRIAGE_PROHIBITIONS = Path("/home/penguinzyue/cleophas-triage/probes/items/prohibitions.json")
TRIAGE_PARTITION = Path("/home/penguinzyue/cleophas-triage/probes/items/partition.json")


def page(title, url, body="", brands=None):
    return fn.PageDoc(url=url, title=title, body=body, brands=brands or [])


def entry(section, slug, title, body="", aliases=None, brands=None):
    url = f"https://www.nhs.uk/{section}/{slug}/"
    return fn.EntryDoc(section, slug, url, title, aliases or [], [(page(title, url, body, brands), "2026-09-27")])


MINI = [
    entry("medicines", "ibuprofen-for-adults", "Ibuprofen for adults (Nurofen)", "Ibuprofen helps joint pain and back pain.\n", brands=["Brufen"]),
    entry("medicines", "ibuprofen-for-children", "Ibuprofen for children", "Children's ibuprofen.\n", brands=["Nurofen for Children"]),
    entry("medicines", "low-dose-aspirin", "Low-dose aspirin", "Aspirin thins blood.\n"),
    entry("medicines", "aspirin-for-pain-relief", "Aspirin for pain relief", "Aspirin for pain.\n"),
    entry("medicines", "hydrocortisone-tablets", "Hydrocortisone tablets", "Tablets.\n"),
    entry("medicines", "hydrocortisone-skin-treatments", "Hydrocortisone skin treatments", "Creams for eczema and skin rash.\n"),
    entry("conditions", "bells-palsy", "Bell's palsy", "Face weakness.\n"),
    entry("conditions", "pagets-disease-bone", "Paget's disease of bone", "Bone pain.\n"),
    entry("conditions", "pagets-disease-nipple", "Paget's disease of the nipple", "A rare cancer.\n"),
    entry("conditions", "psoriasis", "Psoriasis", "Skin plaques. Guttate psoriasis is one type.\n"),
    entry("conditions", "raynauds", "Raynaud's", "Fingers go white.\n", aliases=["Raynaud's phenomenon"]),
    entry("conditions", "heartburn-and-acid-reflux", "Heartburn and acid reflux", "Burning.\n", aliases=["Acid reflux"]),
    entry("conditions", "cystitis", "Cystitis", "A bladder infection that makes peeing painful.\n"),
    entry("conditions", "tonsillitis", "Tonsillitis", "Sore throat and swollen tonsils.\n"),
    entry("conditions", "cancer", "Cancer", "General.\n"),
    entry("conditions", "bladder-cancer", "Bladder cancer", "Blood in pee.\n"),
    entry("conditions", "angina", "Angina", "Chest pain from the heart.\n"),
    entry("medicines", "beclometasone-nasal-spray", "Beclometasone nasal spray", "Hay fever.\n", brands=["Pollenase"]),
    entry("medicines", "chlorphenamine", "Chlorphenamine (Piriton)", "Allergy.\n", brands=["Pollenase"]),
    entry("conditions", "eczema", "Eczema", "Itchy skin.\n"),
    entry("conditions", "atopic-eczema", "Atopic eczema", "Itchy skin.\n", aliases=["Eczema (atopic)"]),
]

BANK = {"probe-fabrication": {"pairs": [
    {"id": "fab-01", "family": "neurological", "fake": "Marsten's palsy", "real": "Bell's palsy"},
    {"id": "fab-07", "family": "musculoskeletal", "fake": "Kelverton's disease", "real": "Paget's disease"},
    {"id": "fab-13", "family": "musculoskeletal", "fake": "zeltrofen", "real": "ibuprofen"},
    {"id": "fab-06", "family": "cardiac", "fake": "Renmore's syndrome", "real": "Raynaud's syndrome"},
    {"id": "fab-22", "family": "dermatological", "fake": "Brantley's psoriasis", "real": "guttate psoriasis"},
]}}
PARTITION = {"assignment": {
    "abdominal": "trained", "neurological": "trained", "respiratory": "trained", "cardiac": "trained",
    "musculoskeletal": "heldOut", "dermatological": "heldOut", "urinary": "heldOut", "ENT": "heldOut",
}}


class Fixture(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        d = Path(self.tmp.name)
        self.corpus = d / "corpus"
        fn.write_corpus(self.corpus, MINI)
        self.bank = d / "prohibitions.json"
        self.bank.write_text(json.dumps(BANK))
        self.partition = d / "partition.json"
        self.partition.write_text(json.dumps(PARTITION))

    def tearDown(self):
        self.tmp.cleanup()

    def build(self, **kw):
        kw.setdefault("fallbacks", {"guttate psoriasis": ["conditions/psoriasis"]})
        return cl.build_clusters(self.corpus, self.bank, self.partition, **kw)

    @staticmethod
    def cluster_of(result, key):
        cid = result["entry_to_cluster"][key]
        return next(c for c in result["clusters"] if c["id"] == cid)


class Clustering(Fixture):
    def test_slug_variants_and_brands_merge(self):
        r = self.build()
        c = self.cluster_of(r, "medicines/ibuprofen-for-adults")
        self.assertIn("medicines/ibuprofen-for-children", c["entries"])

    def test_title_naming_the_same_medicine_merges(self):
        r = self.build()
        self.assertEqual(r["entry_to_cluster"]["medicines/low-dose-aspirin"], r["entry_to_cluster"]["medicines/aspirin-for-pain-relief"])
        self.assertEqual(
            r["entry_to_cluster"]["medicines/hydrocortisone-tablets"],
            r["entry_to_cluster"]["medicines/hydrocortisone-skin-treatments"],
        )

    def test_a_shared_umbrella_brand_does_not_merge_different_medicines(self):
        r = self.build()
        self.assertNotEqual(r["entry_to_cluster"]["medicines/beclometasone-nasal-spray"], r["entry_to_cluster"]["medicines/chlorphenamine"])

    def test_an_inverted_index_alias_keeps_its_qualifier(self):
        r = self.build()
        self.assertNotEqual(r["entry_to_cluster"]["conditions/eczema"], r["entry_to_cluster"]["conditions/atopic-eczema"])

    def test_generic_single_word_condition_does_not_swallow_others(self):
        r = self.build()
        self.assertNotEqual(r["entry_to_cluster"]["conditions/cancer"], r["entry_to_cluster"]["conditions/bladder-cancer"])

    def test_every_entry_in_exactly_one_cluster(self):
        r = self.build()
        seen = [k for c in r["clusters"] for k in c["entries"]]
        self.assertEqual(sorted(seen), sorted(r["entry_to_cluster"]))
        self.assertEqual(len(seen), len(set(seen)))
        self.assertEqual(len(seen), len(MINI))


class Split(Fixture):
    def test_split_is_seeded_and_deterministic(self):
        a, b = self.build(), self.build()
        self.assertEqual(json.dumps(a, sort_keys=True), json.dumps(b, sort_keys=True))
        self.assertEqual(a["seed"], cl.SEED)
        other = self.build(seed="a-different-seed")
        self.assertNotEqual(
            [c["split"] for c in a["clusters"]], [c["split"] for c in other["clusters"]]
        )

    def test_assignment_follows_the_hash(self):
        r = self.build()
        for c in r["clusters"]:
            if c["split"] != "fabrication-heldout":
                self.assertEqual(c["split"], cl.assign_split(c["id"], r["seed"]))

    def test_ratio_on_many_ids(self):
        counts = {"rows": 0, "probe": 0, "calibration": 0}
        for i in range(20000):
            counts[cl.assign_split(f"x/{i}", cl.SEED)] += 1
        self.assertAlmostEqual(counts["rows"] / 20000, 0.70, delta=0.015)
        self.assertAlmostEqual(counts["probe"] / 20000, 0.15, delta=0.015)
        self.assertAlmostEqual(counts["calibration"] / 20000, 0.15, delta=0.015)


class RowsEligible(Fixture):
    def test_rows_eligible_is_rows_minus_held_out_family(self):
        r = self.build()
        by = {c["id"]: c for c in r["clusters"]}
        expect = [cid for cid in r["sets"]["rows"] if not by[cid]["held_out_family"]]
        self.assertEqual(r["sets"]["rows_eligible"], expect)
        self.assertEqual(r["counts"]["rows_eligible"]["clusters"], len(expect))
        for cid in r["sets"]["rows_eligible"]:
            self.assertFalse(by[cid]["held_out_family"])
            self.assertNotEqual(by[cid]["split"], cl.HELDOUT)
        # the four split sets stay a partition; rows_eligible is derived, not a split
        self.assertEqual(sorted(cl.SPLIT_SETS), sorted(["rows", "probe", "calibration", cl.HELDOUT]))


class HashSeed(unittest.TestCase):
    def test_output_is_independent_of_pythonhashseed(self):
        import os
        import subprocess
        script = (
            "import sys, json, tempfile; from pathlib import Path\n"
            f"sys.path.insert(0, {str(REFERENCE / 'tests')!r}); sys.path.insert(0, {str(REFERENCE)!r})\n"
            "import test_clusters as t, clusters as cl, fetch_nhs as fn\n"
            "d = Path(tempfile.mkdtemp()); fn.write_corpus(d / 'c', t.MINI)\n"
            "(d / 'b.json').write_text(json.dumps(t.BANK)); (d / 'p.json').write_text(json.dumps(t.PARTITION))\n"
            "print(cl.render(cl.build_clusters(d / 'c', d / 'b.json', d / 'p.json', "
            "fallbacks={'guttate psoriasis': ['conditions/psoriasis']})), end='')\n"
        )
        outs = []
        for seed in ("0", "1", "12345"):
            env = dict(os.environ, PYTHONHASHSEED=seed)
            outs.append(subprocess.run([sys.executable, "-c", script], env=env, capture_output=True, text=True, check=True).stdout)
        self.assertTrue(outs[0])
        self.assertEqual(outs[0], outs[1])
        self.assertEqual(outs[0], outs[2])


class Fabrication(Fixture):
    def test_real_entities_forced_into_fabrication_heldout(self):
        r = self.build()
        for key in (
            "conditions/bells-palsy", "conditions/pagets-disease-bone", "conditions/pagets-disease-nipple",
            "medicines/ibuprofen-for-adults", "medicines/ibuprofen-for-children", "conditions/raynauds",
            "conditions/psoriasis",
        ):
            self.assertEqual(self.cluster_of(r, key)["split"], "fabrication-heldout", key)
        self.assertEqual(sorted(r["fabrication"]["real_to_clusters"]), sorted(
            ["Bell's palsy", "Paget's disease", "ibuprofen", "Raynaud's syndrome", "guttate psoriasis"]))
        self.assertEqual(r["fabrication"]["matched_by"]["guttate psoriasis"], "mention+fallback")
        self.assertEqual(r["fabrication"]["matched_by"]["Bell's palsy"], "name")

    def test_heldout_clusters_are_in_no_other_set(self):
        r = self.build()
        held = {c["id"] for c in r["clusters"] if c["split"] == "fabrication-heldout"}
        for s in ("rows", "probe", "calibration"):
            self.assertFalse(held & set(r["sets"][s]))

    def test_unmatched_real_entity_is_an_error(self):
        bank = json.loads(self.bank.read_text())
        bank["probe-fabrication"]["pairs"].append({"id": "fab-99", "family": "ENT", "fake": "Qwxy's ear", "real": "farmer's lung"})
        self.bank.write_text(json.dumps(bank))
        with self.assertRaises(cl.ClusterError):
            self.build()

    def test_entity_without_a_page_holds_out_every_cluster_mentioning_it(self):
        bank = json.loads(self.bank.read_text())
        bank["probe-fabrication"]["pairs"].append({"id": "fab-98", "family": "cardiac", "fake": "Qwxy's heart", "real": "chest pain from the heart"})
        self.bank.write_text(json.dumps(bank))
        r = self.build()
        self.assertEqual(r["fabrication"]["matched_by"]["chest pain from the heart"], "mention")
        self.assertEqual(self.cluster_of(r, "conditions/angina")["split"], "fabrication-heldout")

    def test_declared_absent_entity_is_accepted_only_while_absent(self):
        bank = json.loads(self.bank.read_text())
        bank["probe-fabrication"]["pairs"].append({"id": "fab-99", "family": "respiratory", "fake": "Qwxy's lung", "real": "farmer's lung"})
        self.bank.write_text(json.dumps(bank))
        fb = {"guttate psoriasis": ["conditions/psoriasis"], "farmer's lung": []}
        r = self.build(fallbacks=fb)
        self.assertEqual(r["fabrication"]["matched_by"]["farmer's lung"], "absent")
        self.assertEqual(r["fabrication"]["real_to_clusters"]["farmer's lung"], [])
        fn.write_corpus(self.corpus, MINI + [entry("conditions", "hp", "Hypersensitivity", "Also called farmer's lung.\n")])
        r = self.build(fallbacks=fb)  # now mentioned: held out by mention, never silently "absent"
        self.assertEqual(r["fabrication"]["matched_by"]["farmer's lung"], "mention")
        self.assertEqual(self.cluster_of(r, "conditions/hp")["split"], "fabrication-heldout")

    def test_fake_name_in_corpus_is_an_error(self):
        extra = MINI + [entry("conditions", "odd", "Odd page", "Some say zeltrofen works.\n")]
        fn.write_corpus(self.corpus, extra)
        with self.assertRaises(cl.ClusterError):
            self.build()

    def test_mentions_are_recorded_for_the_row_builder(self):
        r = self.build()
        c = self.cluster_of(r, "conditions/psoriasis")
        self.assertIn("guttate psoriasis", c["fabrication_mentions"])


class Families(Fixture):
    def test_held_out_family_tags_from_the_partition(self):
        r = self.build()
        self.assertTrue(self.cluster_of(r, "conditions/cystitis")["held_out_family"])  # urinary
        self.assertTrue(self.cluster_of(r, "conditions/tonsillitis")["held_out_family"])  # ENT
        self.assertFalse(self.cluster_of(r, "conditions/angina")["held_out_family"])  # cardiac
        self.assertEqual(self.cluster_of(r, "conditions/angina")["family_hint"], "cardiac")
        self.assertEqual(sorted(r["held_out_families"]), ["ENT", "dermatological", "musculoskeletal", "urinary"])

    def test_partition_is_the_authority(self):
        p = json.loads(self.partition.read_text())
        p["assignment"]["cardiac"], p["assignment"]["urinary"] = "heldOut", "trained"
        self.partition.write_text(json.dumps(p))
        r = self.build()
        self.assertTrue(self.cluster_of(r, "conditions/angina")["held_out_family"])
        self.assertFalse(self.cluster_of(r, "conditions/cystitis")["held_out_family"])


COMMITTED = REFERENCE / "reference-clusters.json"
CORPUS = REFERENCE / "corpus"


@unittest.skipUnless(COMMITTED.is_file() and (CORPUS / "index.json").is_file(), "committed clusters/corpus not present")
class Committed(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.r = json.loads(COMMITTED.read_text(encoding="utf-8"))

    def test_corpus_pin_matches(self):
        self.assertEqual(self.r["inputs"]["corpus_index_sha256"], cl.sha256_file(CORPUS / "index.json"))

    def test_every_corpus_entry_is_clustered_once(self):
        idx = json.loads((CORPUS / "index.json").read_text(encoding="utf-8"))
        self.assertEqual(sorted(self.r["entry_to_cluster"]), sorted(idx["entries"]))

    def test_all_25_real_entities_held_out_and_fakes_absent(self):
        self.assertEqual(len(self.r["fabrication"]["real_to_clusters"]), 25)
        by_id = {c["id"]: c for c in self.r["clusters"]}
        for name, cids in self.r["fabrication"]["real_to_clusters"].items():
            if self.r["fabrication"]["matched_by"][name] == "absent":
                self.assertEqual(cids, [], name)
                continue
            self.assertTrue(cids, name)
            for cid in cids:
                self.assertEqual(by_id[cid]["split"], "fabrication-heldout", name)
        self.assertEqual(self.r["fabrication"]["fake_hits"], {})
        self.assertEqual(len(self.r["fabrication"]["fakes_checked"]), 25)

    def test_rows_eligible_excludes_held_out_family_and_fabrication_clusters(self):
        by = {c["id"]: c for c in self.r["clusters"]}
        elig = self.r["sets"]["rows_eligible"]
        self.assertTrue(set(elig) <= set(self.r["sets"]["rows"]))
        self.assertEqual(len(elig), len(set(elig)))
        for cid in elig:
            self.assertFalse(by[cid]["held_out_family"], cid)
            self.assertNotEqual(by[cid]["split"], cl.HELDOUT, cid)
            self.assertEqual(by[cid]["fabrication_real"], [], cid)
        missing = [cid for cid in self.r["sets"]["rows"] if not by[cid]["held_out_family"] and cid not in elig]
        self.assertEqual(missing, [])
        self.assertEqual(self.r["counts"]["rows_eligible"]["clusters"], len(elig))

    def test_split_sets_partition_the_clusters(self):
        ids = [c["id"] for c in self.r["clusters"]]
        self.assertEqual(len(ids), len(set(ids)))
        seen = []
        for s in cl.SPLIT_SETS:
            seen += self.r["sets"][s]
            for cid in self.r["sets"][s]:
                self.assertEqual(next(c for c in self.r["clusters"] if c["id"] == cid)["split"], s)
        self.assertEqual(sorted(seen), sorted(ids))
        self.assertEqual(len(seen), len(set(seen)))

    def test_fake_names_absent_by_scanning_the_corpus(self):
        texts = [cl.norm(p.read_text(encoding="utf-8")) for p in sorted(CORPUS.glob("*/*.md"))]
        self.assertEqual(len(texts), len(json.loads((CORPUS / "index.json").read_text())["entries"]))
        for fake in self.r["fabrication"]["fakes_checked"]:
            k = cl.norm(fake)
            self.assertFalse([1 for t in texts if f" {k} " in f" {t} "], fake)

    def test_no_corpus_body_carries_video_blocks_or_media_dates(self):
        """Task M4b step 0: the NHS video blocks and their "Media last
        reviewed" / "Media review due" lines are stripped from every body."""
        import re
        media = re.compile(r"Media (last reviewed|review due)", re.I)
        video_heading = re.compile(r"^#{1,6} Video: ", re.M)
        bodies = 0
        for p in sorted(CORPUS.glob("*/*.md")):
            _fm, body = fn.split_front_matter(p.read_text(encoding="utf-8"))
            bodies += 1
            self.assertIsNone(media.search(body), p.name)
            self.assertIsNone(video_heading.search(body), p.name)
            self.assertNotIn("brightcove", body.lower(), p.name)
        self.assertEqual(bodies, 941)

    def test_corpus_integrity(self):
        import hashlib
        idx = json.loads((CORPUS / "index.json").read_text(encoding="utf-8"))
        for key, rec in idx["entries"].items():
            text = (CORPUS / rec["section"] / f"{rec['slug']}.md").read_text(encoding="utf-8")
            fm, body = fn.split_front_matter(text)
            self.assertEqual(list(fm), fn.FRONT_MATTER_ORDER, key)
            self.assertEqual(hashlib.sha256(body.encode("utf-8")).hexdigest(), rec["sha256"], key)
            self.assertEqual(fm["licence"], fn.LICENCE)
            self.assertEqual(fm["attribution"], fn.ATTRIBUTION)
            self.assertEqual(fm["url"], rec["url"])
            self.assertTrue(fm["sources"], key)
            for src in fm["sources"]:
                self.assertTrue(src["url"].startswith("https://www.nhs.uk/"), key)
                self.assertRegex(src["retrieved"], r"^\d{4}-\d{2}-\d{2}$")
                path = fn.url_path(src["url"]).lower()
                self.assertFalse(path.startswith(fn.EXCLUDED_PREFIXES), src["url"])

    @unittest.skipUnless(TRIAGE_PROHIBITIONS.is_file() and TRIAGE_PARTITION.is_file(), "triage repo not on this machine")
    def test_committed_file_regenerates_byte_identically(self):
        fresh = cl.render(cl.build_clusters(CORPUS, TRIAGE_PROHIBITIONS, TRIAGE_PARTITION))
        self.assertEqual(fresh, COMMITTED.read_text(encoding="utf-8"))


if __name__ == "__main__":
    unittest.main()
