#!/usr/bin/env python3
"""Phase 1h Task M4b — the committed contract files `tools/reference/build/
chunks.jsonl` and `titles.json` (built by `kpack-cli build-reference`; the
Rust test `crates/kpack-cli/tests/reference_pack.rs` proves they are exactly
what the committed corpus builds). These checks re-derive, in Python, what
the triage repo relies on:

- every `content_sha` from its row (the documented JSON rule) and the pack
  `content_sha256` from the ordered shas;
- the header's input pins against the committed corpus and clusters file;
- one title entry per corpus page, rows in pack order, rows agreeing with
  their title entry;
- step 0: no chunk carries the NHS video blocks or their media dates;
- A28/A33: none of the fabrication bank's 25 fake names occurs in any chunk
  (M4a's corpus check, re-asserted over the chunks with clusters.norm).

    python3 -m pytest -q tools/reference/tests/test_reference_build.py
"""

from __future__ import annotations

import hashlib
import json
import re
import sys
import unittest
from pathlib import Path

REFERENCE = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REFERENCE))

import clusters as cl  # noqa: E402

BUILD = REFERENCE / "build"
CHUNKS = BUILD / "chunks.jsonl"
TITLES = BUILD / "titles.json"
KEYS = ["chunk_id", "doc_id", "slug", "title", "section_path", "locator", "url", "retrieved_at", "text",
        "token_count", "content_sha"]


def content_sha(row: dict) -> str:
    s = json.dumps([row["text"], row["section_path"], row["locator"], row["title"]],
                   ensure_ascii=False, separators=(",", ":"))
    return hashlib.sha256(s.encode("utf-8")).hexdigest()


@unittest.skipUnless(CHUNKS.is_file() and TITLES.is_file(), "committed build files not present")
class ReferenceBuild(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.lines = CHUNKS.read_text(encoding="utf-8").splitlines()
        cls.rows = [json.loads(l) for l in cls.lines]
        cls.titles = json.loads(TITLES.read_text(encoding="utf-8"))
        cls.header = cls.titles["header"]

    def test_every_content_sha_and_the_pack_content_sha(self):
        shas = []
        for row in self.rows:
            self.assertEqual(row["content_sha"], content_sha(row), row["chunk_id"])
            shas.append(row["content_sha"])
        pack_sha = hashlib.sha256("".join(s + "\n" for s in shas).encode("ascii")).hexdigest()
        self.assertEqual(pack_sha, self.header["content_sha256"])
        self.assertEqual(self.header["chunks"], len(self.rows))

    def test_rows_have_the_contract_fields_in_order(self):
        for line in self.lines[:200] + self.lines[-200:]:
            self.assertEqual(list(json.loads(line)), KEYS)

    def test_header_pins_match_the_committed_inputs(self):
        h = self.header
        self.assertEqual(h["schema"], "cleophis/reference-titles/v1")
        self.assertEqual((h["pack_id"], h["pack_file"]), ("reference-uk-v1", "reference-uk-v1.kpack"))
        self.assertEqual(h["corpus_index_sha256"], cl.sha256_file(REFERENCE / "corpus" / "index.json"))
        self.assertEqual(h["clusters_sha256"], cl.sha256_file(REFERENCE / "reference-clusters.json"))
        self.assertEqual(h["source_date_epoch"], 1790380800)  # 2026-09-26T00:00:00Z, the corpus fetch date

    def test_one_title_per_page_and_rows_in_pack_order(self):
        idx = json.loads((REFERENCE / "corpus" / "index.json").read_text(encoding="utf-8"))["entries"]
        entries = self.titles["titles"]
        self.assertEqual(len(entries), len(idx))
        self.assertEqual(self.header["titles"], len(idx))
        self.assertEqual(sorted(f"{e['section']}/{e['slug']}" for e in entries), sorted(idx))
        self.assertEqual([e["doc_id"] for e in entries], list(range(1, len(entries) + 1)))
        self.assertEqual([(e["slug"], e["section"]) for e in entries],
                         sorted((e["slug"], e["section"]) for e in entries))
        self.assertEqual([r["chunk_id"] for r in self.rows], list(range(1, len(self.rows) + 1)))
        by_doc = {e["doc_id"]: e for e in entries}
        prev = 0
        for r in self.rows:
            self.assertGreaterEqual(r["doc_id"], prev)
            prev = r["doc_id"]
            e = by_doc[r["doc_id"]]
            self.assertEqual((r["slug"], r["title"], r["url"], r["retrieved_at"]),
                             (e["slug"], e["title"], e["url"], e["retrieved_at"]))
            self.assertTrue(r["url"].startswith("https://www.nhs.uk/"))
            self.assertRegex(r["retrieved_at"], r"^\d{4}-\d{2}-\d{2}$")
            self.assertTrue(r["text"].strip())
            self.assertLessEqual(r["token_count"], 512)
        self.assertEqual(set(by_doc), {r["doc_id"] for r in self.rows}, "every page has a chunk")

    def test_merged_chunks_respect_the_limits(self):
        """Round 2: the curated merge post-pass (~150 tokens, 256 at most;
        a chunk over 256 is a single unmerged block)."""
        self.assertEqual((self.header["merge_target_tokens"], self.header["merge_max_tokens"]), (150, 256))
        for r in self.rows:
            if "\n\n" in r["text"]:
                self.assertLessEqual(r["token_count"], 256, r["chunk_id"])

    def test_title_variants_include_the_core_name_and_the_bare_title(self):
        """Round 2: clusters.py's own core() and parenthetical-free title are
        variants (or already the title / another variant, by clusters.norm)."""
        for e in self.titles["titles"]:
            have = {cl.norm(x) for x in [e["title"], *e["variants"]]}
            core = cl.core(e["title"], e["section"])
            bare = " ".join(cl._PAREN.sub(" ", e["title"]).split())
            for want in (core, bare):
                self.assertTrue(want in e["variants"] or cl.norm(want) in have, (e["title"], want))

    def test_no_chunk_carries_video_blocks_or_media_dates(self):
        media = re.compile(r"Media (last reviewed|review due)", re.I)
        for r in self.rows:
            self.assertIsNone(media.search(r["text"]), r["chunk_id"])
            self.assertNotIn("brightcove", r["text"].lower(), r["chunk_id"])
            self.assertFalse(r["section_path"].split(" > ")[-1].startswith("Video: "), r["chunk_id"])

    def test_no_fake_name_occurs_in_any_chunk(self):
        fakes = json.loads((REFERENCE / "reference-clusters.json").read_text(encoding="utf-8"))["fabrication"]["fakes_checked"]
        self.assertEqual(len(fakes), 25)
        texts = [f" {cl.norm(r['title'] + ' ' + r['section_path'] + ' ' + r['text'])} " for r in self.rows]
        for fake in fakes:
            k = cl.norm(fake)
            self.assertFalse([1 for t in texts if f" {k} " in t], fake)
            first = k.split()[0]  # the distinctive first word ("marsten", "zeltrofen") too
            self.assertFalse([1 for t in texts if f" {first} " in t], fake)


if __name__ == "__main__":
    unittest.main()
