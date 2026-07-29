# Handoff — gen-6 → gen-7 (Phase 5, mid-flight)

Written at a deliberate stopping point, **one item short of my assigned
target**, because starting thermal detection would likely have ended
mid-feature — the failure mode steering and I both named in advance. Three
generations have made deliberate exits work; this continues that.

**State: `mobile/p1-alpha` @ `745233c`, tree clean, `porcelain_exit 0`.**

---

## Read these first

1. `docs/superpowers/verification-milestone-mobile-p1.md` — the ledger. It is
   the institutional memory, and Phase 2's section is dense with lessons that
   were paid for. Read the ⚑ headings at minimum.
2. `docs/superpowers/specs/2026-07-24-mobile-p1-brief.md` — standing orders.
3. `docs/ops/acceptance-coverage.md` — decision **D-6**, the guard, and the
   three bugs it caught in itself. Read before touching the guard.
4. `docs/ops/phase-4-banked-design.md` — **Phase 4 is deferred by founder
   directive. Do not start it.** The file exists so its reasoning is not
   re-derived.

## What is DONE and gated

| | |
|---|---|
| Phases 0–2 | complete. Desktop suite **346/0, zero warnings**, commit-attributable at `2305fc1` |
| Phase 3 | complete. `secure_store` seam + AndroidKeyStore AES-256-GCM |
| 2.2 native completions | metered download policy + `ACTION_SEND` share sheet |
| D-3 unification | canonical placement, bare allows tightened |
| **D-6 guard** | built, and demonstrated failing |
| **5.3** | `mobile-check` CI workflow |

`npm test` **76/76**. The founder holds the CP3 APK
(`a3fd92dddf1f0f61a19d715b37d310f566539ea271a50754d7e61458690efd24`) with the
runbook at `docs/ops/cp3-founder-checkpoint.md`.

## YOUR NEXT TASK — thermal detection, and the guard has already specified it

**This is the one item I did not reach, and it is unusually well-specified**
because the guard's manifest is a contract. `acceptance-coverage.py` will flip
**A7** from `ASSERTED-BUT-UNBUILT` to `ok` when the tree contains:

```
fn detect_thermal_*   (or fn thermal_*)   — Rust
"thermal-notice"                          — a quoted event name
```

Those exact patterns are what you must satisfy. Run the guard before you start
(it exits 1 today, with A7 red) and after (A7 must go green **with no edit to
the guard**). **Record both runs in the ledger as a pair** — steering's
instruction, and the pairing is the evidence the guard works. Negative control
is already banked; you are producing the positive control.

Why it comes before 5.2: **A7's soak test asserts the notice fires.** Write the
suite first and it asserts nothing. That is the D-6 trap, and 5.2 is its native
habitat.

Design not yet started. The shape implied by the spec (§8/H6): detect a
sustained tokens/sec collapse during generation → emit an event → an honest UI
notice. Per **D-3**, the *decision* (what counts as throttling) is pure logic
whose failure is silent, so it belongs where tests run, not behind
`cfg(android)`. Per the D-3 **amendment**, whatever you extract needs the
cfg'd `allow(dead_code)` on its `mod` declaration — ask *who calls this on the
platform where the tests run; if only the tests, it needs the allow.*

## Then, in order

1. **5.1 remainder** — `FLAG_SECURE` toggle + setting (**default OFF**;
   screenshots are the user's right, §5.3); the real foreground service with
   `specialUse` and its justification, started/stopped around `chat_stream`;
   download-complete notification (`POST_NOTIFICATIONS` already in the
   manifest).
2. **5.2 suites** — write the harnesses and founder checklists. Everything
   except the release items must run on a **debug** build; mark release items
   **Phase-4-gated**, not "pending". A1/A2/A3 are red in the guard and they are
   yours to turn green — A3 in particular means making the Stage-5 probes
   *runnable*, not describing them.
3. **5.4** stays founder-blocked (3 devices; they own one).

## Conventions that will bite you if nobody says them

- **Predict the test count before every gate**, and make it *discriminate* —
  map each possible number to a distinct cause. Steering runs the Windows
  suite; you cannot (`tauri → tao → dbus → libdbus-sys` needs system headers,
  no sudo — **this is not `keyring`**, a correction that cost a ledger entry).
- **Freeze protocol.** When steering says "freeze", hold **all** edits and HEAD
  operations until "thawed" — *including* work they approve mid-freeze. An
  approval granted during a freeze takes effect at thaw. I breached this once;
  it cost a gate run. Design at `docs/ops/gate-freeze-marker-design.md` would
  make it structural.
- **An absent result is not a negative finding.** `git status --porcelain` that
  times out returns exit 124 with empty output; a build piped through `tail`
  writes nothing until it finishes; `head` truncates a caller list. **Count
  before you delete**, and the less mechanical the checker behind the target,
  the more that matters.
- **A check's silence is only evidence within its competence.** `cargo ndk
  check` cannot see desktop-dead code and vice versa; their silences are not
  additive.
- **Scripts emit provenance; agents do not report it.** `build-android-apk.sh`
  writes a `.provenance` sidecar. It saved a delivery once already.
- **Verify against the artifact**, never the config that was meant to produce
  it. `verify-apk.py` encodes this, including the dex checks for all four
  Rust-only Kotlin entry points.
- Digests **full-length, never retyped**. `CARGO_TARGET_DIR` must be
  WSL-native. Never touch `feat/english-tutor-demo`.

## The thing I would most want you to internalise

Three times in one day I failed to apply a rule I could recite — and the guard
I built to catch unbuilt requirements shipped three bugs where **it was
satisfied by prose, by a TODO, and finally by its own source**. The last one
appeared only when the file was *committed*, so the demonstration I had already
banked and reported was taken in a state the guard would never be in again.

The conclusion this track keeps arriving at from new directions: **knowing a
rule is not applying it, and only a mechanism closes that gap.** Prefer the
structural fix — a file over a message, a script step over a habit, a contract
over a convention. And when you build a check, **make it fail in front of you
before you believe a green.**
