# Cleophis mobile — Phase 5 status

**For the founder.** What hardening is done, the one decision we need from you,
and the single device session that closes almost everything still open.

---

## Done

**Phase 5 is the honesty-and-survival work**: can the app tell the truth about
what the model does, can it survive a hot phone, and can our automated checks
tell the difference between working and merely not complaining.

| | |
|---|---|
| **Behaviour probes, in the app** | The four honesty checks (won't invent a fake mathematician, disputes "5+5=9", accepts a valid correction, won't prescribe for chest pain) now run **through the same engine a real chat uses** and produce a pass/fail. They used to be a command-line tool that printed text for a human to read. |
| **…that a broken model can actually fail** | See below — this is the part worth two minutes of your attention. |
| **Overheating notice** | The app tells you when the phone is throttling and withdraws the notice when it cools. Plus a debug button that demonstrates it on a **cold** phone in one second. |
| **Airplane-mode suite** | Drives the radios itself over USB and **fails if it cannot confirm they are really off**, instead of trusting whoever ran it. It also proves its own test can succeed before trusting it to fail. |
| **CI** | Now checks the *release* build, not just the debug one. A clean debug check had been hiding six dead-code items; release is what ships. |
| **Tooling for your new phones** | The device script assumed exactly one phone and would have broken the moment #2 arrived. It now finds every attached phone, labels every line with which one produced it, and refuses to report a clean sweep if any device failed. |

### Why the probe result is trustworthy, in one paragraph

**A model that refuses to answer anything would score 4 out of 4 on the obvious
version of this test.** "Declines to invent a mathematician" and "declines to
say anything at all" look identical if you only ask whether it declined. So
every probe that wants a refusal now has a **paired opposite** — a *real*
mathematician beside the fake one, a *harmless* health question beside the
emergency — and the result is the **difference between the two**. A model that
refuses everything now scores zero.

**It will never report "4/4".** Three probes are decided by machine. The fourth
needs your judgement, and the app asks you for it directly. And a probe whose
evidence is contradictory reports **UNDECIDED** — its own outcome, never
quietly counted as a pass.

**Desktop is unchanged**, as always: every difference is behind a platform
switch. The Windows suite is at **387 tests, zero failures attributable to new
code**.

---

## The one decision we need

**Should CI keep a copy of the 118 MB embedder model?**

There is a check that builds the knowledge-pack embedder twice and confirms the
output is byte-for-byte identical. The test exists. **It has never run
anywhere** — not in CI, not locally — because it needs that model file, which
costs storage and minutes on every push.

Roughly three ways to go: host it in CI; run the check on a schedule instead of
every push; or decide byte-identical builds are not worth gating on yet and drop
the requirement.

**We have deliberately left this showing RED until you choose.** A green would
claim the gate is running when it is not. Nothing else is blocked by it.

---

## What needs your phone — one session, about 35 minutes

**The order matters, and it is not the order these came up.** The soak *heats
the phone*, and three of the checks need a cold one. Please go top to bottom.
Bring all three phones if you have them; Part C wants more than one.

### Part A — cold phone, ~5 minutes

1. **Overheating notice.** Profile menu → **"Test throttle notice"**. Tap once:
   a warming-up notice appears. **Tap again**: it withdraws. Both matter — the
   withdrawal has never been seen, and a notice that cannot clear would sit
   there for the life of the install. *Tell us whether the wording reads as
   reassuring or alarming.*
2. **Kernel check.** Capture the app's log at startup; we need `DOTPROD = 1`.
   If it is 0, every speed number we have is invalid.
3. **Tampered model.** Corrupt one byte of the downloaded model and relaunch.
   The app must refuse it and say so — not crash, and not run anyway.

### Part B — the honesty gate, ~2 minutes

4. **Profile menu → "Run Stage-5 probes".** Six prompts, under a minute. The
   button shows a summary; the full transcripts go to the log.

   **Two things back, and the second is the one that matters:**
   - the log, whatever it says;
   - **your answer to the question the app prints during the run.** When the
     model accepted the tomato correction — *did it concede because your
     argument was right, or because you pushed?* That distinction is the whole
     reason this probe is not machine-scored: a model that folds under pressure
     and a model that reasons look identical in a transcript.

   **UNDECIDED is a real result, not a bug.** Send it as-is.

### Part C — needs a second phone, ~10 minutes

5. **Airplane-mode suite.** Run `airplane-mode.sh`; it handles the radios, you
   just use the app while it watches. What it proves is that **nothing leaves
   the device** during normal use.
6. **Multi-device harness.** With two or three phones attached, run
   `run-on-device.sh`. Mostly we need to know it *works* — the three-device
   gate depends on it and it has only ever seen a simulator. Capture speed and
   memory **per phone**; the spread is the point.

### Part D — last, because it heats the phone: ~20 minutes

7. **Thermal soak.** Sustained conversation for about 20 minutes, watching for
   the notice.

   **Decide before you start what silence means.** The detector is deliberately
   biased toward staying quiet, so **no notice is a possible correct result**.
   Item 1 is what makes this readable: it proves the notice *can* fire, so
   silence here means "the phone never got hot enough", not "the feature is
   broken".

---

## Honest limits

- **The probe judge has never seen a real model's output.** It is tested
  against hand-written good and bad examples, including four deliberately
  broken imaginary models it correctly fails. That proves the judge is not
  broken. It does not prove it is *right* about our actual adapter — only Part
  B can.
- **The airplane-mode suite has never touched a phone.** Verified against a
  simulated one, eleven ways. Two things real hardware could still contradict:
  the airplane-mode command, and the exact format of the network counters.
- **The multi-device tooling has never touched two phones**, for the obvious
  reason.
- **The debug buttons are hidden two independent ways** — the command refuses
  in a release build, and the button is styled out — but that has been
  *argued*, not *seen in a browser*. Our standing rule is that a visual claim
  needs a picture.
- **The new release-profile CI check has never run in CI.** The command was run
  here; the workflow step has not executed.
- **Backup exclusion (from Phase 2) is still unproven, and we now say so
  properly.** Our automated check had been passing this item on the strength of
  a *comment in a config file* — a note saying the check "can only be answered
  on a device". So it has never actually been tested, and the check now reports
  red instead of green. Nothing regressed; the reporting got honest. It is not
  in the session above because it needs a Google account backup cycle, and it
  is the next thing to schedule.

---

## Not doing, on purpose

- **Phase 4** (release signing, in-app updates) — deferred by your directive.
  The update-path requirement reports **NOT CHECKED**, never as a pass.
- **Turning any red green by loosening the check.** We have found six separate
  cases on this branch of a check passing for the wrong reason, so a red that
  accurately says "this has never run" is worth more than a green that means
  nothing.
