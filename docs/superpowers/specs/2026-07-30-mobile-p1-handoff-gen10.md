# Handoff — gen-10 → gen-11 (end of Phase 5's buildable work)

Written outside the worktree first, deliberately, so it survives the branch and
the session.

**State: branch `mobile/p1-alpha`, tree clean, `porcelain_exit 0`. READ HEAD
YOURSELF** — this document names no commit, because a hash in a message is a
fact with two homes and it has gone stale twice in ten minutes on this track.
`git log --oneline main..HEAD` is the authority.

---

## ⚠ YOUR FIRST ACT: read this section before you plan anything

**There may be no work for you yet, and that is the correct state — not a gap
you should fill.**

Phase 5's buildable work is done. What remains is blocked on things no agent
can do:

| item | blocked on |
|---|---|
| A1 — determinism CI | a **founder decision** about hosting a 118 MB GGUF in CI |
| A5 — backup-leak | a **Google account backup cycle**, not a phone in hand |
| 5.4 — three-device gate | **devices that have not arrived** |
| everything unproven | **the founder's device session** (see below) |

An agent spawned into a founder-blocked phase will invent work if nobody tells
it not to. **Do not.** If steering has given you a task, do that. If it has
not, your first act is to **ask steering what the boundary is** — and "nothing
until the founder acts" is a legitimate answer to receive.

Three things you may be tempted by, and should not start unilaterally:

- **Phase 4** — deferred by founder directive. Not "pending".
- **Turning A1 or A5 green.** Both are red because something real is untested.
  This branch has found **six** separate cases of a check passing for the wrong
  reason; a red that accurately says "this has never run" is the asset.
- **Running the founder's device session yourself.** You cannot, and the
  document that asks for it is written and approved.

---

## What is true right now

**Acceptance (`docs/superpowers/mobile-tools/acceptance-coverage.py`):**

```
ok            A2  A3  A4  A7
NOT-CHECKED   A6   (Phase 4 deferred — reported under its own token, never as a pass)
FAIL          A1   (deliberate: CI does not run the suite)
FAIL          A5   (correct: the backup-leak test has never been run)
```

**Desktop gate: 387, prediction exact, all 21 probe fixtures observed BY
NAME.** One failure, `cloud::rest::tests::create_portal_session_request_shape`,
the documented mock family, isolation-green in 0.32 s → GREEN per protocol.
Zero warnings. **Verify before you trust it**: run
`git diff --name-only <gated-commit> HEAD` and confirm no
`.rs`/`.toml`/`.lock`/`.kt`/`.js` path matches. It described HEAD when written;
that claim expires silently, which is exactly how 366/0 nearly misled gen-10.

**aarch64 clean at debug AND `--release`.** Both, always — see Habits.

---

## What gen-10 built

**A3 — the Stage-5 adjudicator.** `src-tauri/src/engine_inproc/probes.rs`
(pure `probe_verdict`, D-3, gated out of release) and
`chat_cmds::chat_stage5_probe` (debug-only, runs six arms through
`chat_complete → run_turn` — the shipped path). 21 fixtures.

The one thing you must not "improve" without understanding it:

> **The per-arm measure does not need to be ACCURATE, only CONSISTENTLY
> APPLIED ACROSS BOTH ARMS.** The comparison does the discriminating the
> measure cannot.

`asserted_particulars` counts kinds of claim off word lists and is frankly
crude — that is *fine*, because the same crudeness lands on Rendell and on
Riemann. **The failure mode to guard against is not imprecision; it is a change
that makes the measure behave differently on the two arms** — special-casing a
phrase that only appears in declines, tuning a threshold against the fake arm
alone. Any of those converts a differential into two unrelated measurements
**and every fixture still passes.** The note is in the file, above the
fixtures, deliberately in your path.

**A1's guard scoping.** Manifest elements may now be `(root, regex)` tuples,
scoped to a SEARCH root, and all tuples sharing a root must be satisfied by
**one file**. A1 asks for determinism *CI*, so its clauses live in
`.github/workflows`.

**Comment stripping.** The guard no longer treats comments as implementation.

**`--release` step** in `mobile-check.yml`.

**`run-on-device.sh` multi-device.** Discovers every attached phone, targets
with `-s`, attributes every line to a serial, collects failures and exits
non-zero naming them.

---

## 🔴 The two findings you are most likely to trip over

**1. A guard clause is satisfiable by a comment.** Run 2's rule was "make
patterns definition-shaped so prose cannot satisfy them". That is false: **a
doc comment that quotes a symbol is prose satisfying a definition-shaped
pattern, because a pattern cannot distinguish quoting from defining.** Measured
— the guard reported `ok  A3 implemented` against a file of three comment lines
and no implementation.

Fixed at the **corpus**, not the pattern, and the reason that was safe is worth
carrying: **stripping narrows and is monotone** (it can only turn `ok` → `FAIL`),
so it cannot manufacture a false green. Generalisation:

> **When a matcher cannot distinguish two things, fix the corpus, not the
> pattern.**

Residual, stated so you do not read the fix as total: **a trailing comment can
still satisfy a clause.** Closing that needs a per-language tokenizer.

**2. A clause must name a BEHAVIOUR, never an ARTIFACT.** The guard reads
contents, so a clause naming a *filename* is not a content fact. One principle
explains both of A2's pathologies: false green off usage comments before
stripping, false red after. Swapped for `verify_egress_works *\(` — the
positive control, so A2's green now depends on the clause that makes the suite
mean anything.

---

## Owed, not claimed — the honest core, and it is short

1. **A3's adjudicator has never run against a model.** Guard `ok`, 21 fixtures
   green, release clean — none of that is evidence that six real prompts
   produce anything it handles. It is **`#chatStatusPill`-shaped** until the
   founder session says otherwise, and the assertion inventory says so in its
   own row. *Do not report A3 as proven.*
2. **A2 has never touched real hardware.** Mock only, eleven ways. `cmd
   connectivity airplane-mode` and the `dumpsys netstats detail` row format are
   what a real phone could contradict.
3. **`run-on-device.sh`'s multi-device path has never seen two phones.** Nine
   mock scenarios; `adb -s` against real hardware is unverified.
4. **The debug buttons' invisibility on desktop and in release is argued from
   CSS specificity and the cfg, not seen in a browser at two widths** (D-5's
   standard). Gen-9 owed the same for Q1; gen-10 added a second button and owes
   it too.

Plus: **the `--release` CI step has never executed in CI** (the command was run
locally, the workflow step has not), and **the determinism suite has never run
anywhere** — not CI, not locally; this host cannot build it (no system
libclang, and `LIBCLANG_PATH` points at a Python package with no builtin
headers). *Do not improvise a sysroot: when the check is about reproducibility,
the environment is part of the claim.*

---

## ⚑ A debt gen-10 created deliberately: `discover_devices` has two homes

`discover_devices` and `adbs` are **byte-identical** in `airplane-mode.sh` and
`run-on-device.sh`. Copied rather than extracted, because `airplane-mode.sh` is
A2's evidence and its eleven-scenario mock no longer exists — hoisting two
functions would have re-opened that verification for a tidiness win.

That is a real D-4 cost and it has already shown up: with the harness
untracked, A2's `discover_devices` clause **still matches**, carried by the
other script. A2 requires all three clauses so the item still reds correctly,
but the clause is no longer evidence about the harness.

**The right time to fix it is the change that next gives A2's harness a mock**,
so both scripts can be re-verified together. Not before.

---

## A1's three options, as they were framed for the founder

The founder has been asked, in `docs/ops/phase5-founder-status.md`, to choose:

1. **Host the 118 MB GGUF in CI** — simplest, ongoing storage and minutes.
2. **Run the check on a schedule** rather than every push — cheaper, slower to
   catch a regression.
3. **Drop the requirement** — decide byte-identical builds are not worth gating
   on yet, and amend §11 so the spec stops asserting it.

**Whatever comes back, one thing does not change:** the suite's two tests are
`#[ignore]`d and `return` early with `eprintln!("SKIPPED: model missing…")`, so
even `--ignored` makes them **pass while asserting nothing**. Any runner must
refuse to start without the GGUF **and** reject output containing `SKIPPED:` or
a zero test count. That is D-6's own boundary — *the acceptance item that warns
about unexecuted suites can itself be satisfied by an unexecuted suite.*

---

## The founder device session — written, approved, one ~35-minute ask

`docs/ops/phase5-founder-status.md`. Seven items that accumulated over weeks,
now **four ordered parts**. The ordering is load-bearing and the reason is
stated in the document so nobody optimises it away: **the soak heats the phone
and three checks need it cold.**

A: cold — throttle-notice two-tap, `[kernels]` DOTPROD, tampered-file probe.
B: the Stage-5 in-app run, **including probe 3's human question**.
C: needs a 2nd phone — airplane-mode suite, multi-device harness.
D: last — the ~20-minute thermal soak.

**When the results come back, the first thing to do is calibrate A3's
adjudicator against the real transcripts** — that is the item's remaining risk,
and the clauses most likely to need it are probe 4's dose detection and probe
1's particulars threshold.

---

## Habits (not optional; each has a body count)

`source /home/penguinzyue/cleophis-mobile-logs/mobile-env.sh` before any
cargo/gradle work. **`docs/superpowers/mobile-tools/run-logged.sh` for anything
long** — never pipe something whose exit code matters. Gradle compile for any
`gen/android` change. **`--release` as well as debug for anything
`debug_assertions`-gated.** Full-length digests, never retyped. **Predict the
count AND the warning state before a gate, mapping each outcome to a distinct
cause** — and note that on both of gen-10's gates the prediction's value was
the **names observed**, not the number.

**Freeze protocol is now SIX rules.** Rule 6, adopted this generation: **the
ACK is a fresh `git status --porcelain` taken AFTER the freeze arrives,
reported with its exit code** — a receipt and a state verification in one
action, superseding the recollection-based reading of rules 3–5.

**And the one this generation would add**, because it cost the most:

> **Before writing down WHY something failed, run the smallest thing that
> isolates that mechanism from every other reason the same command could
> fail.**

Three mis-attributions in one session, **all of which looked exactly like
passes**: a `set -e` claim asserted in a commit message *and* a source comment
and then disproved by two `bash -c` runs; a control that "confirmed" a fix while
failing on an empty-array expansion; a control where the old script died on the
mock's serial guard before reaching the mechanism under test. The psychological
half is the part that generalises: **a drafted sentence actively resists being
checked, because it already sounds finished.**

---

## Tools, not rules, where they exist

Per the standing lesson that a handoff handing over a *rule* hands over
precisely the thing that already failed — for each warning above, the tool:

| instead of remembering | use |
|---|---|
| "don't launder exit codes" | `mobile-tools/run-logged.sh` |
| "check the release profile" | the `--release` step now in `mobile-check.yml` |
| "comments aren't implementation" | the guard strips them now |
| "state the tree is clean" | rule 6's `porcelain` ACK |
| "test the adjudicator" | `#[path]` the real `probes.rs` into a bare crate — the crate needs host libclang, so this is the only way to run it here, and it is the **actual file**, not a copy |
