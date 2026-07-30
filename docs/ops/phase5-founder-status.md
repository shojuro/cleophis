# Phase 5 — status, and the one device session it needs

**Branch `mobile/p1-alpha`. Written 2026-07-30.** For the founder. Everything
here is either *built and verified*, *built and not yet proven on hardware*, or
*deliberately not done*. The three are kept apart on purpose — this project's
characteristic failure is the confident-looking record, not the incomplete one.

---

## 1. The short version

Phase 5 is hardening: the checks that decide whether the app is honest, whether
it survives a hot phone, and whether CI can tell. Of the seven acceptance
criteria in the spec (A1–A7):

| | criterion | state |
|---|---|---|
| A1 | determinism in CI | **red on purpose** — waiting on one decision from you |
| A2 | airplane-mode suite | built; **never run on a phone** |
| A3 | behavioural probes in-app | built; **never run against a real model** |
| A4 | kill/restore | built and passing |
| A5 | backup-leak | **red, and newly so** — see §4 |
| A6 | update path | not started; Phase 4 is deferred by your directive |
| A7 | thermal notice | built; **never seen on a hot phone** |

**The pattern is the point.** Four of these are built and unproven on hardware,
which is not a gap in the work — it is the same gap, and **one device session
closes all of it.** That session is §5, and it is about 35 minutes.

---

## 2. What got built, in plain terms

**A3 — the honesty probes now run inside the app.** The four Stage-5 probes
(invent-a-mathematician, "5+5=9", accept-a-correction, medical boundary) used to
exist only as a command-line tool that printed transcripts for a human to read.
They now run through the *same engine path a real chat uses* and produce a
machine verdict.

The part worth understanding, because it changes what the number means: **a
model that refuses everything would score 4/4 on the obvious version of this
test.** "Declines to invent a mathematician" and "declines to answer anything"
look identical if you only ask whether it declined. So each probe that wants a
refusal now has a **paired control** — a *real* mathematician beside the fake
one, a *harmless* health question beside the emergency — and the verdict is the
**difference**. A model that refuses everything now scores 0.

**It will never report "4/4".** Three probes are machine-decided; the fourth
(does it accept a correction?) is screened by machine and **needs your
judgement**, because the machine can see *that* it conceded and never *why*.
And a probe whose evidence is contradictory is reported as UNDECIDED — its own
result, never quietly counted as a pass.

**A7 — the "your phone is warming up" notice**, plus a two-tap debug button that
demonstrates it on a *cold* phone in about a second. Without that button, a
20-minute soak that produces no notice is uninterpretable: it means either "the
detector works and your phone stayed cool" or "the detector is broken", and
those are the same observation.

**A2 — the airplane-mode harness** drives the radios itself over USB and
**fails if it cannot confirm they are actually off**, rather than asking a human
to remember. It also proves its own test can succeed before trusting it to fail.

**Tooling for your second and third phones.** `run-on-device.sh` assumed exactly
one phone and would have broken the moment the new devices arrived. It now finds
every attached phone, labels every line with which one produced it, and refuses
to report a clean sweep if any device failed.

---

## 3. THE ONE DECISION WE NEED FROM YOU

**A1 — determinism in CI. Should CI host the 118 MB embedder model?**

The check verifies that building the knowledge-pack embedder twice produces
byte-identical output. The test suite exists. **It has never run anywhere** —
not in CI, not locally — and CI cannot run it without a copy of a 118 MB model
file, which costs storage and minutes on every push.

The options are roughly: host it in CI (simplest, ongoing cost); run the check
only on a schedule rather than every push (cheaper, slower to catch a
regression); or decide determinism is not worth gating on yet and drop the
criterion from the spec.

**A1 is deliberately left showing RED until you choose.** A green would say the
gate is running when it is not, and the red is what keeps the question visible.
Nothing else is blocked by it.

---

## 4. What is owed, stated plainly

These are claims we have *not* made, listed because the temptation is always to
let them slide by unmentioned.

1. **A3's adjudicator has never seen a real model's output.** It is tested
   against hand-written examples of good and bad behaviour, including four
   deliberately broken imaginary models it correctly fails. That proves the
   judge is not broken. It does not prove the judge is *right* about a real
   adapter, and only your device session can.
2. **A2 has never touched a phone.** Verified against a simulated one, eleven
   ways. Two things a real device could still contradict: the airplane-mode
   command and the exact format of the network counters we read.
3. **The multi-device tooling has never touched two phones**, for the obvious
   reason. Verified against a simulator covering one, two and three.
4. **The debug buttons are hidden by two independent mechanisms** (the command
   refuses in a release build; the button is styled out) — but that invisibility
   has been *argued*, not *seen in a browser*. Our standing rule says a visual
   claim needs a picture.
5. **The new release-profile CI check has never run in CI.** The command was run
   here; the workflow step has not executed.
6. **A5 (backup-leak) just went red, and it was wrong before.** The guard was
   being satisfied by a *comment* in a config file — a note saying the check
   "can only be answered on a device". So a criterion asserting the conversation
   database stays out of Android backups has never actually been tested, and now
   says so. Nothing regressed; the reporting got honest. The real test is in the
   device session below.

---

## 5. THE DEVICE SESSION — one sitting, ~35 minutes, ordered so it works

This has grown to seven items across several weeks. It is **one session**, not a
list, and **the order matters**: the soak heats the phone, and three of the
checks require a cold one. Please do them top to bottom.

Bring **all three phones** if you have them; items 5 and 6 want more than one.

### Part A — cold phone, about 5 minutes

1. **Throttle notice (A7).** Profile menu → **"Test throttle notice"**. Tap it:
   a "your phone is warming up" notice should appear. **Tap it again**: it
   should withdraw. Both edges matter — the withdrawal has never been seen, and
   a notice that cannot clear would stay stuck for the life of the install.
   *Tell us: did the wording read as reassuring, or alarming?*
2. **Kernel check.** With the phone plugged in, capture the app's log at
   startup. We need the `[kernels]` line to show **DOTPROD = 1**. If it is 0,
   every speed number we have is invalid.
3. **Tampered-file probe.** Corrupt one byte of the downloaded model and relaunch.
   The app must refuse to load it and say so, rather than crash or run anyway.

### Part B — the honesty gate (A3), about 2 minutes

4. **Profile menu → "Run Stage-5 probes".** Six prompts, under a minute. The
   button shows a summary; the full transcripts and verdicts go to the log.

   **We need two things from you here, and the second is the important one:**
   - the log, whatever it says;
   - **your answer to the question the app prints during the run**: when the
     model accepted the tomato correction — *did it concede because your
     argument was right, or because you pushed?* That distinction is the entire
     reason this probe is not machine-scored. A model that folds under pressure
     and a model that reasons look identical in a transcript.

   If any probe reports **UNDECIDED**, that is a real result and not a bug —
   send it as-is.

### Part C — needs a second phone, about 10 minutes

5. **Airplane-mode suite (A2).** Run `airplane-mode.sh`. It handles the radios
   itself; you exercise the app while it watches. What we are proving is that
   **nothing leaves the device** during normal use.
6. **Multi-device harness.** With two or three phones attached, run
   `run-on-device.sh`. We mainly need to know it *works* — this is the tooling
   the three-device gate depends on and it has only ever seen a simulator.
   Capture speed and memory **per phone**; the spread across devices is the
   point.

### Part D — last, because it heats the phone: ~20 minutes

7. **Thermal soak (A7).** Sustained conversation for about 20 minutes and watch
   for the notice.

   **Decide before you start what the outcome means**, because otherwise it is
   uninterpretable: the detector is deliberately biased toward staying quiet, so
   **no notice is a possible correct result.** Item 1 is what makes this
   readable — it proves the notice *can* fire, so silence here means "the phone
   never got hot enough", not "the feature is broken". The log now records the
   generation rate throughout, which is the evidence that was missing.

---

## 6. What we are deliberately NOT doing

- **Phase 4 (release signing, update path)** — deferred by your directive. A6
  cannot be checked until a release channel exists, and it is reported as
  **NOT-CHECKED**, never as a pass.
- **Turning A1 or A5 green by loosening their checks.** Both are red because
  something real is untested. A red that accurately says "this has never run" is
  worth more than a green that means nothing — and we have found six separate
  cases on this branch of a check passing for the wrong reason, so this is not
  a hypothetical worry.
