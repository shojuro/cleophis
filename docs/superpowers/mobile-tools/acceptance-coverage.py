#!/usr/bin/env python3
"""Acceptance-coverage guard: does anything IMPLEMENT what §11 asserts?

Four times on this branch a test or acceptance criterion asserted behaviour no
task ever built:

  1. A7 soak test asserts "throttle notice fires appropriately" -- nothing
     detects throttling or emits a notice.
  2. A4 kill-restore demanded an intact transcript -- nothing flushed partial
     turns until 1.4.
  3. A5 backup-leak asserted the DB absent from the backup set -- the rules
     named four directories the data is not in.
  4. The §2 download policy (unmetered default + charge notice) was absent
     entirely until the 2.2 native completions.

All four were caught by HUMAN PLAN REVIEW. That is the problem this script
exists to fix: every other recurring failure on this branch has been converted
into a check -- the provenance sidecar, verify-apk.py's dex symbols, the D-3
bare-allow audit -- and this one was still enforced by attention, which the
ledger records failing three times in a single day on rules the agent could
recite.

It is the D-4 family: a requirement lives in the spec, in the code, and in a
test -- three homes, no canonical link. The reason THIS class escaped while
hazards did not is that §11's bullets were the only requirement set in the spec
with no identifiers. Giving them A1-A7 is what makes this script possible.

WHAT IT DOES NOT DO -- read before trusting a green run:

  * It checks EXISTENCE, not correctness. Whether the throttle notice is right
    is the test's job; whether anything claims the requirement at all is what
    nobody was checking.
  * IT HAS A KNOWN BLIND SPOT: a symbol can exist and be wired to nothing.
    `#chatStatusPill` existed for the entire project -- hard-coded in
    index.html, never written by any code -- and a symbol-existence check would
    have passed it green while the founder could not find the engine state at
    all. This catches ABSENCE; only the phase-entry assertion inventory
    (decision D-6, part 3) catches INERT PRESENCE. A green run here is not a
    substitute for that pass.
"""
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]

# id -> (evidence patterns [regex, all must match somewhere], consequence)
#
# Same shape as verify-apk.py's KOTLIN_EXPECTED for the same reason: the
# consequence is named AT THE POINT OF FAILURE, because none of these present
# as "an acceptance item is unimplemented" -- they present as a suite that
# passes while asserting nothing.
# ⚠ PATTERNS MUST BE DEFINITION-SHAPED, NOT TOPIC WORDS.
#
# The second bug this file caught in itself. After markdown was excluded, A7
# STILL reported implemented, on two independent false matches:
#
#   * `throttl` hit the offline sign-in throttle, the progress-tick throttle
#     and a SQLite write-throttling comment -- a generic word for an unrelated
#     mechanism.
#   * `thermal` hit AndroidManifest.xml and InferenceService.kt comments that
#     say the notice "lands in Phase 5.1" -- i.e. COMMENTS PROMISING THE
#     FEATURE COUNTED AS THE FEATURE. A guard satisfiable by a TODO is not a
#     guard.
#
# So every pattern below is shaped like a DEFINITION or a quoted literal --
# `fn name(`, `"event-name"` -- which a prose mention or a TODO cannot satisfy
# by accident. A useful side effect: the manifest is now a CONTRACT. Each
# pattern names the exact symbol the implementing phase must create, so 5.1
# knows what will turn A7 green before it writes a line.
ACCEPTANCE = {
    'A1': (
        [r'cargo test -p kpack-embed', r'build_determinism'],
        'the determinism gate is claimed but no runner invokes it -- CI would be '
        'green without ever having run the suite it is named for',
    ),
    'A2': (
        [r'\bfn +airplane_|"airplane"|airplane-mode\.sh|run-airplane'],
        'the airplane-mode suite asserts offline behaviour with nothing to drive '
        'it -- A2 would be satisfied by a human remembering to turn radios off',
    ),
    'A3': (
        [r'fake[- ]entity', r'medical'],
        'the Stage-5 probes exist only as prose: a probe set nobody can run '
        'cannot fail, so "4/4" means whatever the reader wants it to',
    ),
    'A4': (
        [r'fn +checkpoint_partial', r'fn +finalize_partial'],
        'kill-restore asserts an intact transcript while nothing flushes partial '
        'turns -- the suite would pass on an empty DB',
    ),
    'A5': (
        [r'exclude domain="root"', r'\bbmgr\b'],
        'the backup-leak test asserts the DB is absent while the exclusion rules '
        'name directories the data is not in -- green for the wrong reason',
    ),
    'A6': (
        [r'fn +check_app_update'],
        'the update-path test asserts an update flow that does not exist',
    ),
    'A7': (
        [r'fn +(detect_)?thermal_|"thermal-notice"'],
        'the soak test asserts a throttle notice no code emits: the suite passes '
        'by asserting nothing. NOTE the two false matches this pattern was '
        'tightened to exclude -- unrelated rate-limiting `throttl`, and comments '
        'promising the feature in a later phase',
    ),
}

# Items that cannot be decided by this script and MUST NOT be reported as
# passing. release-config-audit.md's closing rule: "an item that could not be
# checked is recorded as NOT CHECKED -- this project's failure mode is the
# confident-looking record, not the incomplete one."
NOT_CHECKABLE = {
    'A6': 'needs Phase 4 signing (deferred by founder directive) -- no update '
          'flow can exist until the release channel does',
}

# Implementation and runners ONLY. Prose is deliberately excluded -- see below.
SEARCH = ['src-tauri/src', 'src', 'src-tauri/gen/android/app/src/main',
          'docs/superpowers/mobile-tools']

# ⚠ THE GUARD'S OWN FIRST BUG, fixed here and recorded because it is the exact
# failure the guard was built to catch.
#
# The first version searched `docs/ops` and every `.md`, and its first run
# reported **A7 implemented** -- while nothing whatsoever detects thermal
# throttling. It had matched the words "thermal" and "throttle" in the ledger
# and in the CP3 checkpoint doc: PROSE DESCRIBING THE REQUIREMENT, counted as
# evidence of the requirement being met.
#
# That is circular, and it is the check-that-cannot-fail family in its purest
# form: the spec would satisfy itself, and the guard would go green precisely
# because someone had written the requirement down. So: markdown is never
# evidence. Code is evidence, and an executable runner is evidence; a document
# saying the thing should exist is the assertion, not the implementation.
EVIDENCE_SUFFIXES = ('.rs', '.js', '.mjs', '.kt', '.xml', '.py', '.sh', '.toml',
                     '.json', '.pro', '.gradle', '.kts')


def haystack() -> str:
    """Every tracked IMPLEMENTATION file in the search roots, concatenated.

    `git ls-files` rather than a walk: untracked scratch must never satisfy an
    acceptance item, or the guard goes green on a file nobody will review.
    """
    out = []
    for root in SEARCH:
        if not (ROOT / root).exists():
            continue
        files = subprocess.run(['git', 'ls-files', '-z', root], cwd=ROOT,
                               capture_output=True, text=True).stdout.split('\0')
        for rel in filter(None, files):
            if not rel.endswith(EVIDENCE_SUFFIXES):
                continue
            try:
                out.append((ROOT / rel).read_text(errors='ignore'))
            except (OSError, UnicodeDecodeError):
                continue
    return '\n'.join(out)


def main() -> int:
    hay = haystack()
    unbuilt, satisfied, skipped = [], [], []

    # Non-short-circuiting on purpose: report EVERY unbuilt item in one run.
    # A guard that stops at the first failure turns one review pass into N.
    for aid, (patterns, consequence) in sorted(ACCEPTANCE.items()):
        if aid in NOT_CHECKABLE:
            skipped.append((aid, NOT_CHECKABLE[aid]))
            continue
        missing = [p for p in patterns if not re.search(p, hay, re.I)]
        if missing:
            unbuilt.append((aid, missing, consequence))
        else:
            satisfied.append(aid)

    print("== acceptance coverage (spec §11 A1-A7) ==")
    for aid in satisfied:
        print(f"ok            {aid} implemented")
    for aid, why in skipped:
        # Deliberately its own token: this is NOT a pass, and it must not be
        # mistaken for one by a human skimming or a grep for 'ok'.
        print(f"NOT-CHECKED   {aid} -- {why}")
    for aid, missing, consequence in unbuilt:
        print(f"FAIL          {aid} ASSERTED-BUT-UNBUILT (no match: {missing})")
        print(f"                 -> {consequence}")

    print(f"\n{len(satisfied)} implemented, {len(skipped)} not checked, "
          f"{len(unbuilt)} asserted-but-unbuilt")
    if unbuilt:
        print("VERDICT: FAIL -- an acceptance item names no implementation.")
        print("Build it, or amend §11 so the spec stops asserting it.")
        return 1
    print("VERDICT: PASS (existence only -- see the blind spot in this file's "
          "docstring; the phase-entry inventory is what catches inert presence)")
    return 0


if __name__ == '__main__':
    sys.exit(main())
