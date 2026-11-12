#!/usr/bin/env python3
"""Tests for the KNOWN-GAP status in acceptance-coverage.py.

Stdlib unittest only (the CI runner has no pytest) and synthetic inputs only:
the real tree is never read, so these cannot be satisfied or broken by what
happens to be committed.
"""
import contextlib
import importlib.util
import io
import re
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name('acceptance-coverage.py')
spec = importlib.util.spec_from_file_location('acceptance_coverage', SCRIPT)
ac = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ac)

PRESENT = r'alpha_symbol'
ABSENT = r'never_written_anywhere'


class RunTests(unittest.TestCase):
    def run_main(self, acceptance, not_checkable=None, known_gaps=None,
                 files=('fn alpha_symbol()',)):
        """Run main() against synthetic globals; return (exit code, stdout)."""
        saved = (ac.ACCEPTANCE, ac.NOT_CHECKABLE, ac.KNOWN_GAPS,
                 ac.files_by_root)
        ac.ACCEPTANCE = acceptance
        ac.NOT_CHECKABLE = not_checkable or {}
        ac.KNOWN_GAPS = known_gaps or {}
        ac.files_by_root = lambda: {'src': list(files)}
        out = io.StringIO()
        try:
            with contextlib.redirect_stdout(out):
                code = ac.main()
        finally:
            (ac.ACCEPTANCE, ac.NOT_CHECKABLE, ac.KNOWN_GAPS,
             ac.files_by_root) = saved
        return code, out.getvalue()

    def test_known_gap_unmatched_reports_and_passes(self):
        code, out = self.run_main({'X1': ([ABSENT], 'consequence')},
                                  known_gaps={'X1': 'waits on a decision'})
        self.assertEqual(code, 0)
        self.assertRegex(out, r'(?m)^KNOWN-GAP     X1')
        self.assertIn('waits on a decision', out)
        self.assertNotIn('ok            X1', out)
        self.assertIn('VERDICT: PASS', out)
        self.assertIn('1 known gaps', out)

    def test_known_gap_partially_matched_is_still_a_gap(self):
        code, out = self.run_main({'X1': ([PRESENT, ABSENT], 'c')},
                                  known_gaps={'X1': 'why'})
        self.assertEqual(code, 0)
        self.assertIn('KNOWN-GAP     X1', out)

    def test_known_gap_fully_matched_is_gap_closed_and_fails(self):
        code, out = self.run_main({'X1': ([PRESENT], 'consequence')},
                                  known_gaps={'X1': 'why'})
        self.assertEqual(code, 1)
        self.assertRegex(out, r'(?m)^GAP-CLOSED    X1')
        self.assertIn('remove', out)
        self.assertIn('VERDICT: FAIL', out)

    def test_ordinary_unbuilt_item_still_fails(self):
        code, out = self.run_main({'X1': ([ABSENT], 'consequence')})
        self.assertEqual(code, 1)
        self.assertIn('FAIL          X1 ASSERTED-BUT-UNBUILT', out)

    def test_every_failure_is_reported_in_one_run(self):
        code, out = self.run_main({'X1': ([ABSENT], 'c'),
                                   'X2': ([ABSENT], 'c'),
                                   'X3': ([PRESENT], 'c')},
                                  known_gaps={'X3': 'why'})
        self.assertEqual(code, 1)
        self.assertIn('FAIL          X1', out)
        self.assertIn('FAIL          X2', out)
        self.assertIn('GAP-CLOSED    X3', out)

    def test_not_checkable_unchanged(self):
        code, out = self.run_main({'X1': ([ABSENT], 'c')},
                                  not_checkable={'X1': 'needs signing'})
        self.assertEqual(code, 0)
        self.assertIn('NOT-CHECKED   X1 -- needs signing', out)
        self.assertIn('1 not checked', out)

    def test_implemented_item_is_ok(self):
        code, out = self.run_main({'X1': ([PRESENT], 'c')})
        self.assertEqual(code, 0)
        self.assertIn('ok            X1 implemented', out)
        self.assertIn('1 implemented, 0 known gaps, 0 not checked', out)

    def test_known_gaps_do_not_hide_a_real_failure(self):
        code, out = self.run_main({'X1': ([ABSENT], 'c'),
                                   'X2': ([ABSENT], 'c')},
                                  known_gaps={'X1': 'why'})
        self.assertEqual(code, 1)
        self.assertIn('KNOWN-GAP     X1', out)
        self.assertIn('FAIL          X2', out)

    def test_id_in_both_lists_exits_2(self):
        code, out = self.run_main({'X1': ([ABSENT], 'c')},
                                  not_checkable={'X1': 'a'},
                                  known_gaps={'X1': 'b'})
        self.assertEqual(code, 2)
        self.assertIn('X1', out)
        self.assertNotIn('VERDICT', out)

    def test_known_gap_id_not_in_acceptance_exits_2(self):
        code, out = self.run_main({'X1': ([PRESENT], 'c')},
                                  known_gaps={'NOPE': 'b'})
        self.assertEqual(code, 2)
        self.assertIn('NOPE', out)
        self.assertNotIn('VERDICT', out)


class ShippedConfigTests(unittest.TestCase):
    def test_shipped_lists_are_consistent(self):
        self.assertFalse(set(ac.KNOWN_GAPS) & set(ac.NOT_CHECKABLE))
        self.assertTrue(set(ac.KNOWN_GAPS) <= set(ac.ACCEPTANCE))
        for reason in ac.KNOWN_GAPS.values():
            self.assertTrue(reason.strip())

    def test_this_file_matches_no_acceptance_pattern(self):
        # This file lives in a SEARCH root and is not self-excluded by the
        # guard, so quoting a real pattern here could flip an item green.
        text = Path(__file__).read_text(errors='ignore')
        for item, (clauses, _why) in ac.ACCEPTANCE.items():
            for clause in clauses:
                pattern = clause[1] if isinstance(clause, tuple) else clause
                self.assertIsNone(re.search(pattern, text),
                                  f'{item}: {pattern!r} matches this file')


if __name__ == '__main__':
    unittest.main()
