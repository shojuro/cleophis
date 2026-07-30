# Handoff — gen-8 → gen-9 (Phase 5, mid-flight)

Written outside the worktree deliberately, so it survives whatever happens to
the branch or the session.

**State: `mobile/p1-alpha` @ `7bb86b1f6d6d640f2a4cf77a1b6578e6ece10c19`, tree
clean, `porcelain_exit 0`.** Nothing is half-written. Every item below is either
committed and verified, or explicitly not started.

---

## Read these first

1. `docs/superpowers/verification-milestone-mobile-p1.md` — the ledger. Read
   **Conventions** (two new ⚑ entries at the top are mine) and **D-6**.
2. `docs/superpowers/specs/2026-07-29-mobile-p1-handoff-phase5.md` — gen-6's
   handoff. Still accurate except where this document supersedes it.
3. `docs/ops/acceptance-coverage.md` — read before touching the guard.
4. **`/home/penguinzyue/cleophis-mobile-logs/mobile-env.sh`** — source this
   before any cargo/gradle work. See "Environment" below; it will save you a
   round trip and a false conclusion.

## Environment (this is the first thing that will bite you)

```bash
source /home/penguinzyue/cleophis-mobile-logs/mobile-env.sh
```

Without it, `cargo ndk` exits **1** with "Could not find any NDK" — which looks
exactly like a code failure in a transcript and is not one. That is the
absent-result rule applied to your own tooling.

**Two verification commands, and you need BOTH:**

```bash
# Rust — says NOTHING about Kotlin, resources, or the manifest
cargo ndk -t arm64-v8a -P 24 check -p cleophis --all-targets

# Android — any commit touching gen/android needs this
cd src-tauri/gen/android && ./gradlew :app:compileUniversalDebugKotlin
```

**Never pipe a build through `tail`/`head`.** I did, and the harness reported a
*failed* gradle build as task **exit 0** — that was `tail`'s status. Redirect to
a file, or tee and capture `${PIPESTATUS[0]}`. Also: a piped build writes
nothing until it finishes, so an empty log is not a stalled build.

## What I completed

| | |
|---|---|
| **A7 thermal detection** | `7550f23` — closed end to end, desktop-verified **359/0, zero warnings**, all 13 thermal tests observed by name |
| **D-6 control pair** | `85da25d` — negative + positive banked with a byte-identical guard, one `git add` apart |
| **5.1 foreground service + FLAG_SECURE** | `42a5065` |
| **Guard reconciliation + A3 tightening** | `7bb86b1` |

Gen-7 authored the thermal detector and its 13 tests before dying to an
infrastructure failure; I verified them, found two gaps, closed both. Its module
doc is preserved verbatim and is worth reading before you touch `thermal.rs`.

## THE ONE THING TO INTERNALISE

**A passing check still owes you an attribution.** It is now a Convention.

A7 flipped green *before I committed anything*, on the strength of a single
`emit` call, with the entire detector invisible to the guard. The guard's A7
entry was an OR where every sibling is an AND. That produced a **right verdict
for the wrong reason** — nothing looked amiss, because A7 genuinely was
implemented — and the control pair would have been banked as proof of a causal
link that did not exist.

The countermeasure is cheap: run the check against a state where only the
*suspected cause* is absent, and confirm it names that cause specifically. My
negative control named `fn +(detect_)?thermal_` as the **only** miss while
`"thermal-notice"` still matched. That is what makes the pair evidence rather
than decoration.

Corollary, learned when steering ruled the opposite and I pushed back with
measurements: **the fix for a coverage gap is not to widen coverage under
patterns too loose to survive it.** Tighten first, widen second, or not at all.
Steering verified independently and reversed the ruling. Pushing back *before*
implementing is the behaviour that caught it — do not stop doing that.

## YOUR NEXT TASK — 5.1's last item, and it is BLOCKED ON A DECISION

**Do not build the download-complete notification until steering answers.**

The brief says "`POST_NOTIFICATIONS` already in the manifest". True, and
insufficient: `targetSdk = 36`, and **nothing in the app requests it at
runtime** (I grepped; there is no `requestPermissions` anywhere). On Android 13+
it is therefore never granted.

- The **foreground service is unaffected in substance** — it still runs and
  still protects the turn; only its notification is suppressed. Documented in
  `InferenceService.kt` as survivable, and it genuinely is.
- A **download notification would post into a void** on every shipping device.

Three options are with steering; my recommendation is (3): build the runtime
permission request asking at *first download* (contextual, far likelier to be
granted than a first-launch prompt), and keep the notification as a small
follow-up. The grant is what unlocks both this and the FGS notification's
visibility.

## Then, in the order steering approved

1. **A2 — airplane-mode suite.** Genuinely greenfield and the most
   straightforward of the three. The guard wants `fn airplane_` / `"airplane"` /
   `airplane-mode.sh` / `run-airplane`. Build it **device-agnostic**, not
   A22-specific: the founder is acquiring a second and third device for 5.4.
2. **A3 — bring the ADJUDICATION DESIGN to steering BEFORE implementing.** This
   is an explicit instruction, not a courtesy.

   Scoping already done: `crates/kpack-engine/examples/probe.rs` (183 lines)
   already defines the four Stage-5 probes verbatim and documents its own arm64
   cross-compile, so portability is nearly free. **It has no pass/fail logic at
   all** — it prints transcripts for a human. The cost is entirely in
   adjudication.

   Steering's constraints: a refusal-keyword heuristic is **rejected outright**
   (a model that refuses everything scores 4/4, inverting the property under
   test); whatever you propose must be able to *fail* a broken adapter and
   *pass* a correct one phrased unexpectedly, and you must say how you would
   test that claim; and consider that **some probes may be machine-adjudicable
   and others not** — "two are checkable, two need a human" is a more honest
   design than a uniform heuristic, and a 4/4 that mixes the two is exactly the
   number that means whatever the reader wants.

   The guard now names what you must build: `fn probe_verdict` and `"stage5"`,
   reachable from `src-tauri/src`. That is a contract, decided in advance.
3. **A1(a) — the determinism runner, failing LOUDLY.** The suite already exists
   (`crates/kpack-embed/tests/build_determinism.rs`, `#[ignore]`d, skips when
   the GGUF is absent). Build the runner so an absent model is a **red build,
   not a skip**. Steering's ruling: let A1 sit *red-because-unexecuted* rather
   than *green-because-a-runner-exists*. The CI-model question (118MB GGUF,
   hosting, minutes) is deliberately **not** being sent to the founder yet.

   Why it matters: **the acceptance item that warns about unexecuted suites can
   itself be satisfied by an unexecuted suite.**
4. **5.4** stays founder-blocked (3 devices; they own one).

**Phase 4 is DEFERRED by founder directive. Do not start it.**

## Founder device ask — ACCUMULATE, do not send piecemeal

Steering is explicit that device time is the scarcest resource on this track and
wants one coherent session. Currently queued:

- Stage-5 probe transcripts, to calibrate adjudication against real output
- CP1 leftovers: logcat `[kernels]` DOTPROD=1, TTFT/tok-s/RSS with realistic
  history, the tampered-file probe, Stage-5 4/4
- **New from me:** a ~20-minute thermal soak. It is the only way to see whether
  the detector fires on a genuinely hot phone, and whether the §8 copy reads as
  reassuring or alarming. The detector is biased toward false negatives on
  purpose, so a soak that never fires is a *possible* correct result — decide in
  advance what would distinguish "correctly quiet" from "broken", or the session
  produces an uninterpretable outcome.

## Constraints that have not changed

- Desktop byte-identical; cfg-gated only. Never touch `feat/english-tutor-demo`.
- H1 rendering invariant. D-3 + its amendment (canonical allow on the `mod`
  declaration, cfg'd to mirror the production caller, **never bare**).
- CI is push-only on `mobile/**`. Widening it is a **founder** decision — stop
  and surface, do not tweak the config.
- Founder items surfaced are never attempted.
- Gate protocol v2: a freeze binds **unconditionally**. An approval that arrives
  during a freeze takes effect at thaw, never before. I held four rulings that
  way this session and it cost nothing.
- Digests full-length, never retyped. `CARGO_TARGET_DIR` WSL-native.
- Commit convention `feat(mobile-p1)` / `fix(mobile-p1)`, ending:
  `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`

## Small things that will save you time

- `verify-apk.py`'s method check is a **byte search scoped to the dex holding
  the class**. Never add a generic method name — I nearly added
  `['start','stop']`, which would have been a check that could never fail
  (`Thread.start` is in every dex). Use a unique class descriptor instead; I
  used `InferenceService$Companion`.
- XML comments cannot contain `--`. aapt2 rejects the whole resource with a
  row/column parse error that never mentions the rule.
- The profile menu is the only settings-shaped surface in the app. `.mobile-only`
  forces `display:grid`, which fights `.profilemenu-item`'s `display:block` —
  gate new menu items with their own `.is-mobile #id` rule (D-5: platform class,
  never viewport width).
