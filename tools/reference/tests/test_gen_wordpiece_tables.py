#!/usr/bin/env python3
"""Phase 1h Task M4b — gen_wordpiece_tables.py: the committed Rust tables are a
fresh render of the vendored llama.cpp unicode-data.cpp.

    python3 -m pytest -q tools/reference/tests/test_gen_wordpiece_tables.py
"""

from __future__ import annotations

import sys
import unittest
from pathlib import Path

REFERENCE = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REFERENCE))

import gen_wordpiece_tables as gen  # noqa: E402


@unittest.skipUnless(gen.DEFAULT_SRC.is_file(), "vendored llama.cpp not present")
class Tables(unittest.TestCase):
    def test_committed_rust_tables_are_a_fresh_render(self):
        fresh = gen.render(gen.DEFAULT_SRC.read_bytes())
        self.assertEqual(fresh, gen.DEFAULT_OUT.read_text(encoding="utf-8"))

    def test_tables_keep_llama_cpp_invariants(self):
        t = gen.parse(gen.DEFAULT_SRC.read_text(encoding="utf-8"))
        self.assertEqual(t["flags"][0], (0, 0x0080))
        self.assertEqual(t["flags"][-1][0], 0x110000)
        self.assertIn(0x202F, t["whitespace"])  # narrow no-break space (in the corpus)
        e_acute = [to for first, last, to in t["nfd"] if first <= 0xE9 <= last]
        self.assertEqual(e_acute, [0x65])  # é -> e: llama.cpp's NFD strips the accent
        self.assertIn((0x41, 0x61), t["lowercase"])


if __name__ == "__main__":
    unittest.main()
