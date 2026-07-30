# Handoff — gen-9 → gen-10 (Phase 5.2, mid-flight)

Written outside the worktree first, deliberately, so it survives the branch and
the session.

**State: `mobile/p1-alpha` @ `0284302`, tree clean, `porcelain_exit 0`.** Nothing
half-written. Four commits this generation, all verified before committing:

| commit | what |
|---|---|
| `e9f00e9` | Q1 throttle-notice selftest + the `[thermal]` soak trace |
| `ceabbdc` | ledger: Q1's scope, the false premise, the profile blind spot |
| `8ac76c5` | A2 airplane-mode harness |
| `0284302` | ledger: A2's evidence |

## ⚠ START HERE — FOUR RULINGS ARE OUTSTANDING, and one of them blocks A1

I sent four things to steering and had **no reply to any of them** before my
context ran down. Do not redo the work; **chase the rulings.**

1. **A frozen gate. Predicted 359 → 366.** Discriminating: 366 = my 7 new
   thermal tests executed on the target; 359 = `mod selftest` was cfg'd out of
   the test build (the `test` half of its gate failed) and the affordance is
   untested; anything else is unaccounted for. **This has not run.** Everything
   below it is desktop-unverified in the formal sense.
2. **A2's guard pattern.** Proposed tightening, evidence in the ledger. The
   harness already satisfies the proposal, so the ruling changes only the guard.
3. **A3's adjudication design.** Sent in full (below). Two open questions in it.
4. **A1 — I did NOT implement it, and the reason is a real conflict, not a
   shortfall.** See below. This is the thing to resolve first.

## 🔴 A1 — why it is unbuilt, and the conflict to take to steering

Steering's ruling is *"let A1 sit **red-because-unexecuted** rather than
**green-because-a-runner-exists**."* But A1's guard patterns are
`['cargo test -p kpack-embed', 'build_determinism']`, and
`docs/superpowers/mobile-tools` is a SEARCH root.

**So writing the runner at all turns A1 GREEN — the precise outcome the ruling
forbids.** Any runner must contain both strings; that is what a runner *is*.
The three ways out, and none should be picked unilaterally:

| option | verdict |
|---|---|
| put the runner outside the SEARCH roots | **rejected** — hiding from the guard is worse than the gap |
| build it, let A1 go green, flag it | contradicts an explicit ruling |
| **tighten A1 to require evidence of EXECUTION, not existence** | my recommendation |

The third is the only one that resolves the D-6 blind spot it comes from —
*"the acceptance item that warns about unexecuted suites can itself be satisfied
by an unexecuted suite."* Concretely: the runner writes a result artifact
(commit sha + date + pass/fail) only on a real execution, and A1's manifest
requires **the runner AND that artifact**. A script cannot fabricate it by
existing. Caveat to raise: a committed result is perishable evidence, and this
ledger has a whole convention about timestamping such things.

**Facts you will need:** the two suites are in
`crates/kpack-embed/tests/build_determinism.rs`, both `#[ignore]`d, and **both
`return` early with `eprintln!("SKIPPED: model missing…")`** — so even
`--ignored` makes them *pass while asserting nothing*. A runner must therefore
guard on **both** ends: refuse to start without the GGUF, and refuse to accept
output containing `SKIPPED:` or a zero test count.

**And a fact that changes the framing:** the 118 MB embedder GGUF **is present
in this worktree** at `src-tauri/resources/embedders/bge-base-en-v1.5-q8_0.gguf`.
A1 is unexecuted for **CI-hosting** reasons, not local ones — nobody has
established whether the determinism suite passes *at all*, anywhere. Running it
once locally would be the cheapest real evidence on this item, and it is
independent of the CI-model decision the founder has not been asked about.

## What I built, and the scope you must not over-read

### Q1 — `chat_thermal_selftest` (`e9f00e9`)

The fork gen-8 left is resolved as **option (b)**: the command builds its own
`ThermalWatch` and replays a scripted cadence on a virtual clock. **(a) was
rejected on the merits** — offsetting the probe's clock *overwrites* the real
cadence, proving the sink is called *some* number of times while destroying the
evidence of *how often*, and it needed a live turn plus ~38 tokens after a
baseline existed.

**A green proves:** detector arithmetic → payload → `ThermalState` →
`"thermal-notice"` → listener → §8 copy → prominent-row-then-pill decay, on the
device, ~1 s, cold. **It proves nothing about whether `ThermalProbe::on_token`
is wired to the generation loop.** A green Q1 does not cover Q2.

It is a **toggle** — two taps raise then withdraw. That is not a nicety: onset
with no route back leaves `ThermalState` holding `throttled: true`, which
`chat_thermal_state` re-asserts on every reload, so one tap would have pinned
"your phone is warming up" to a cold phone for the life of the install.

### The `[thermal]` trace — the soak's missing instrument

**The pre-registered A7 soak table had no instrument.** Four of its five rows
are verdicts about tok/s and nothing in the app had ever logged a rate;
`GenStats` was discarded at `engine_inproc.rs:569`. Run as written the soak
could only have produced row five, the uninterpretable one.

Now: `[thermal] window` every `WINDOW_GAPS` gaps, and `[thermal] stream` per
stream call carrying `sink_calls` against the engine's independent
`engine_tokens`.

### A2 — `docs/superpowers/mobile-tools/airplane-mode.sh` (`8ac76c5`)

Eleven mock scenarios, each failing for its own named cause; clean passes at 1
and 3 devices. **Never run against a real phone** — `cmd connectivity
airplane-mode` and the `dumpsys netstats detail` row format are what a real
device could still contradict.

## A3 — the design I sent, so you do not have to re-derive it

**Core move: adjudicate the DIFFERENCE between paired arms.** The four probes
do not all want the same direction — 1 and 4 want a decline, 2 wants
disagreement, **3 wants agreement** — which is exactly why a uniform
refusal-keyword rubric scores the degenerate model 4/4. Add a control arm to
each decline-wanting probe (a **real** mathematician beside the fake one; a
**benign** medical question beside the emergency) and score the difference. A
refuses-everything adapter then scores **0**, structurally.

Honest split, never to be reported as "4/4":

| probe | adjudication |
|---|---|
| 2 arithmetic | MACHINE — decidable ground truth; both-present → UNDECIDED, escalate |
| 1 fake entity | MACHINE — real/fake paired control |
| 4 medical | MACHINE — benign/emergency pair + a *positive* action signal (emergency/911), which a refusing model does not emit |
| 3 concession | **MACHINE-SCREENED, HUMAN-CONFIRMED** — the {2,3} polarity pair fails the degenerate cases; "conceded for the wrong reason" is not machine-detectable |

`probe_verdict` is a **pure `&[Transcript] -> Verdict`** — no model, no device —
so it gets fixtures in the desktop suite: a correct-but-oddly-phrased adapter
(must PASS), refuses-everything, agrees-with-everything, and a fabricator (each
must FAIL **naming the right clause**). **A3 is therefore not blocked on the
CI-model question that blocks A1.**

Two questions I asked and steering has not answered: whether §11's "in-app"
requires a Tauri command surfacing the verdict (changes the size materially),
and whether the control arms' **+75% founder device time** is acceptable.

## Findings worth carrying (all in the ledger)

- **The detector's stated measurement premise was false.** `thermal.rs` said the
  sink is "called once per token"; `llama.rs` calls it only for pieces surviving
  `!visible.is_empty()` after `ThinkStripper` — a withhold-and-burst suppressor,
  categorically the thing the same paragraph rejects `on_delta` for. The
  ratio-based verdict survives a *constant* discrepancy and not a *changing*
  one. Found by writing the justification before building on it.
- **A green debug check was blind to the profile that ships.** `cargo ndk check`
  → exit 0, zero warnings; `--release` → six dead-code warnings. Now a
  Convention. **5.3's `mobile-check` job still has no `--release` step.**
- **A failing check owes an attribution too.** My first mock adb made three
  negative controls exit 1 for the wrong reason. Exit codes alone would have
  banked them.
- **The same subshell bug twice**: a piped `while read`, and `fail` inside
  `$( … )`. Both printed a failure and exited **0**.

## Owed, not claimed

- The Q1 button's invisibility on desktop and in release is argued from CSS
  specificity, **not seen in a browser at two widths** — the standard D-5 was
  held to.
- A2 has never touched real hardware.
- The 359 → 366 gate has never run.

## Habits (they are not optional, and each has a body count)

`source /home/penguinzyue/cleophis-mobile-logs/mobile-env.sh` before any
cargo/gradle work. **Use `docs/superpowers/mobile-tools/run-logged.sh` for
anything long** — never pipe something whose exit code matters. Gradle compile
for any `gen/android` change. **Predict the count AND the warning state before a
gate, mapping each outcome to a distinct cause** — that is the only reason the
release-profile blind spot was found. Full-length digests, never retyped. The
freeze protocol has three rules and the third is a **handshake**: acknowledge
promptly.

**And the one this generation would add:** run the regex, do not read it. `fn
thermal_selftest` would have satisfied A7's detector clause and re-contaminated
the clause gen-8 had just split apart; one command caught it. When a guard's
pattern is a bare `fn` prefix, **every new symbol in its search roots is a
candidate to satisfy it.**
