# The acceptance-coverage guard — design

Closes the "asserted but unbuilt" trap. Built as Phase 5's opening item,
founder-approved. Implementation: `mobile-tools/acceptance-coverage.py`, run as
step 2 of `check-mobile-build.sh`.

---

## The incidents

Four times on this branch, a test or acceptance criterion asserted behaviour
**no task ever built**:

| # | the assertion | what existed |
|---|---|---|
| 1 | **A7** soak: "throttle notice fires appropriately" | nothing detected throttling or emitted a notice — true when written, **closed by 5.1 at `7550f23`** |
| 2 | **A4** kill-restore: "transcript intact, no corruption" | nothing flushed partial turns until 1.4 |
| 3 | **A5** backup-leak: "DB absent from the backup set" | exclusion rules naming four directories the data is not in |
| 4 | §2 download policy: unmetered default + charge notice | absent entirely until the 2.2 native completions |

**All four were caught by human plan review, and by nothing else.** That is the
problem. Every other recurring failure on this branch has been converted into a
check — the provenance sidecar, `verify-apk.py`'s dex symbols, the D-3
bare-allow audit, the `.gate-freeze` marker. This class was still enforced by
attention, which the ledger records failing **three times in a single day** on
rules the agent could recite.

## The diagnosis, and why this class escaped while hazards did not

It is the **D-4 family**: a requirement lives in the spec, in the code, and in
a test — three homes, no canonical link.

The specific reason it escaped: **§11's acceptance bullets were the only
requirement set in the spec with no identifiers.** Hazards are H1–H15, sections
are §n, decisions are D-1…D-6 and are cited from code comments. Acceptance
criteria were unnamed prose, referred to as "the §11 kill-restore test". *A
mapping cannot be checked mechanically when one side has no name.* Giving them
**A1–A7** is the enabling change; nothing else was possible without it.

## ⚑ The manifest is CANONICAL; this document cites it and must not restate it

Adopted after bug 4, whose root cause was **D-4 arriving inside the guard's own
documentation**: A7's contract had two homes — this file said "nothing *emits* a
notice" (an OR), the ledger said detector **and** event name (an AND) — and the
executable manifest quietly implemented the weaker one. Nobody compared them,
because nothing marked either as authoritative.

So, from here:

- **`acceptance-coverage.py`'s `ACCEPTANCE` dict is the single source of truth**
  for what satisfies each item. It is the only statement that is *executed*, so
  it is the only one that cannot drift from behaviour.
- **This document and the ledger explain WHY, cite the manifest for WHAT, and
  never paraphrase a pattern.** Where a contract must be discussed, name the
  item (`A7`) and point at the dict; do not re-describe its patterns in prose.
- Run transcripts quoted below are **historical records of output**, not
  restatements of contract, and are left exactly as printed.

The general form, which is not specific to this script: **a fact with two homes
and no canonical link will drift, and the drift is invisible precisely because
both homes look authoritative.** Pick one, and make the other cite it.

## Mechanism

`ACCEPTANCE = {id: (evidence patterns, consequence-if-missing)}`, deliberately
mirroring `verify-apk.py`'s proven `KOTLIN_EXPECTED` shape: non-short-circuiting
(report every failure in one run — a guard that stops at the first turns one
review pass into N), with the **consequence named at the point of failure**,
because none of these present as "an acceptance item is unimplemented". They
present as a suite that passes while asserting nothing.

Three states, **distinguishable in the output**:

| state | verdict |
|---|---|
| **ASSERTED-BUT-UNBUILT** | `FAIL`, blocking, exit 1 |
| implemented | `ok` |
| **NOT-CHECKED** | reported under its own token, never as a pass |

The third is load-bearing, per `release-config-audit.md`'s closing rule: *an
item that could not be checked is recorded as not checked — this project's
failure mode is the confident-looking record, not the incomplete one.* A6 is
Phase-4-gated and reports that way today.

## Demonstrated capable of failing — first run, before 5.1

```
ok            A4 implemented
ok            A5 implemented
NOT-CHECKED   A6 -- needs Phase 4 signing …
FAIL          A1 ASSERTED-BUT-UNBUILT   (no CI runner invokes the determinism suite)
FAIL          A2 ASSERTED-BUT-UNBUILT   (no airplane-mode harness exists)
FAIL          A3 ASSERTED-BUT-UNBUILT   (the Stage-5 probes exist only as prose)
FAIL          A7 ASSERTED-BUT-UNBUILT   (nothing emits a throttle notice)
VERDICT: FAIL          exit 1
```

A guard first run *after* the gap is closed is a guard nobody has seen fail.
When 5.1 lands the notice, the same command must flip A7 green **with no edit
to the guard** — that pair is the proof.

## 🔬 The guard caught four of its own bugs

Three on its first three runs; the fourth while running the positive control it
was built for. Recorded because they are the same failure the guard exists to
prevent, committed by the guard, which is the strongest possible argument for
the demonstrated-capable-of-failing rule.

**Run 1 — prose counted as implementation.** The first version searched
`docs/` and every `.md`. It reported **A7 implemented** while nothing
whatsoever detects thermal throttling: it had matched the words "thermal" and
"throttle" *in the ledger and in the CP3 checkpoint doc*. Circular — the spec
would satisfy itself, and the guard would go green precisely **because someone
had written the requirement down**. Fixed: markdown is never evidence. Code is
evidence; an executable runner is evidence; a document saying the thing should
exist is the assertion, not the implementation.

**Run 2 — a TODO counted as implementation.** With markdown excluded, A7 *still*
passed, on two independent false matches: `throttl` hit the offline sign-in
throttle, the progress-tick throttle and a SQLite write-throttling comment
(a generic word for unrelated mechanisms), and `thermal` hit
`AndroidManifest.xml` and `InferenceService.kt` comments saying the notice
"lands in Phase 5.1". **Comments promising the feature counted as the feature.**
Fixed: every pattern is now **definition-shaped** — `fn name(`, `"event-name"` —
which a prose mention or a TODO cannot satisfy by accident.

**Run 3 — the manifest satisfied itself, and only after being committed.** The
`ACCEPTANCE` dict lists every pattern as a literal string, so the guard's own
source is a perfect match for its own requirements: it reported **A1, A2 and A7
implemented** on the strength of nothing but itself.

This is the nastiest of the three, because **it did not exist until the file
was committed.** `git ls-files` excludes untracked files, so the
demonstrated-capable-of-failing run — taken, recorded and reported to steering
*before* the commit — was performed in a state the guard would never be in
again. The verdict changed at commit time, with no edit and nothing to notice.
Fixed by excluding the guard's own resolved path: **a manifest of patterns can
never be evidence for the patterns it lists.**

The generalisation worth keeping: **a check whose own source lives inside its
search space is self-satisfying**, and the moment that becomes true may be a
`git add` rather than an edit.

A useful side effect of the run-2 fix: **the manifest became a contract.** Each
pattern names the exact symbol the implementing phase must create, so 5.1 knows
what will turn A7 green before writing a line.

**Run 4 — an OR where every sibling is an AND: a green that was right for the
wrong reason.** Found by gen-8 while running the positive control this guard
was built for. A7's entry was one pattern with a top-level alternation,
`[r'fn +(detect_)?thermal_|"thermal-notice"']`, so **either half satisfied it
alone** — while A1, A4 and A5 use two-element lists the guard requires *all* of.
A bare `emit("thermal-notice", …)` with no detector whatsoever turned A7 green.
Measured, not theorised: with the detector written but still untracked,
`fn +(detect_)?thermal_` matched **none** of the 80 tracked evidence files and
the emit site alone carried A7 to `ok`.

**Why this is the nastiest for the instrument, though the mildest for the
code.** Runs 1–3 produced *wrong verdicts*. Run 4 produced a **right verdict for
the wrong reason** — A7 was genuinely implemented, so nothing looked amiss — and
the control pair would have been banked as "A7 flipped when thermal detection
landed" when the flip **was not attributable to the detector and would have
happened with it deleted.** A contaminated positive control is worse than a
missing one, because it retires the question.

**How it survived: this document and the ledger disagreed.** The line above
banks A7's failure as "nothing *emits* a throttle notice" — an OR reading —
while the ledger's second-order finding states the contract as
`fn detect_thermal_*` **and** `"thermal-notice"`. The manifest implemented the
weaker of the two and nobody compared them. Fixed by splitting A7 into two
required patterns, a strict tightening that cannot turn anything green that was
red.

The generalisation: **a passing check still owes you an attribution.** Ask
*which clause carried the verdict*, not merely whether the verdict was right.
Run 3's lesson was that a check can become self-satisfying at a `git add`;
run 4's is that a check can be satisfiable by strictly less than the requirement
it names, and no amount of staring at a green run will show it.

## The pair is complete — the guard is now a proven instrument

Recorded because the whole design rests on it: a guard demonstrated only failing
is plausible; one demonstrated failing **and** passing, across a change that
should move it and with nothing else different, is proven.

| # | state | A7 |
|---|---|---|
| negative | tightened guard, detector written but **untracked**, HEAD `5b17981` | **FAIL** — missing `fn +(detect_)?thermal_` *only* |
| positive | **byte-identical guard**, detector committed, HEAD `7550f23` | **ok** |

The two runs differ by **one `git add`** and nothing else; `git diff` on the
guard between them returns empty. The negative run naming *only* the detector
pattern — while `"thermal-notice"` already matched — is the direct proof the AND
is load-bearing, since the old OR would have passed that exact state.

## What it deliberately does not do

- **Not a requirements-management system.** Seven items, one spec section, one
  script step. Growth past §11 needs its own argument.
- **Checks existence, not correctness.** Whether the throttle notice is *right*
  is the test's job; whether anything claims the requirement at all is what
  nobody was checking.
- **⚠ It has a known blind spot, which is why the phase-entry inventory is not
  optional.** A symbol can exist and be wired to nothing. `#chatStatusPill`
  existed for the entire project — hard-coded in `index.html`, never written by
  any code — and a symbol-existence check would have passed it green while the
  founder could not locate the engine state at all. **This guard catches
  ABSENCE; only the inventory catches INERT PRESENCE.** Do not let this script's
  existence argue the inventory out of the process.

## Cost

One script, one manifest entry per acceptance item, one step in an existing
runner. No parallel machinery — 5.3's CI job calls the same script.
