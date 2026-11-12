# Medical Triage Adapter — Full Debrief

**Model line:** Qwen3-1.7B LoRA triage adapter (`triage-armb-*-Qwen3-1.7B`)
**Repo:** `~/cleophas-triage` (private, `shojuro/cleophas-triage`)
**Governing document:** `docs/spec.md` — 2,570 lines, rulings **R1–R73**
**As of:** 2026-09-09 · 100 commits
**Status:** **Not shippable.** Best candidate identified. Three ship-blockers open.

---

## 1. Where this started and what changed

The programme began as a **0.6B** proof-of-concept: distil a Qwen3-0.6B from a larger teacher, quantise
to Q4_K_M, run offline on a budget Android, and prove that targeted training makes a small model punch
above its weight.

**Two things changed that plan, both from measurement rather than opinion.**

**First, the 0.6B is not fit for medical triage.** It escalates 83 of 100 emergencies; of the 17 it
misses, 11 get *confident wrong urgency* rather than a refusal (appendicitis with peritonism told *"within
the next 48 hours"*). Three prohibition probes sit at or near zero on it. Both training arms converged on
**exactly 83** emergency recall from two different loss functions, which is evidence the ceiling is the
model, not the method.

**Second, distillation bought nothing.** The pre-registered A/B — GKD versus plain SFT on identical data,
splits, seed, LR, precision, and (verified in the artefact) identical resolved library versions — came
back **72 vs 71, +1.0 pp, two-sided McNemar p = 1.00000.** C1 not met. That was registered in advance as a
*successful* run: ~$3.20 to retire a claim that would otherwise have anchored an unsupportable pitch.

**What did work is the data and the measurement discipline.** The corpus rebuild bought **+20 endpoint
points to both arms** (51 → 71/72). The valuable half of the pipeline is the data work, not the
distillation.

**So the line shipped is the 1.7B**, and the 0.6B is retained as a capability proof: emergency recall
**21 → 83**, beating an *untrained 1.7B* (64) and approaching an *untrained 4B* (87) — a model roughly
seven times its size.

---

## 2. The safety contract

Fixed at the start, never renegotiated. The model **routes; it does not diagnose.**

| disposition | meaning | requirement |
|---|---|---|
| `EMERGENCY` | emergency services / ED now | "do not drive yourself" |
| `CLINICIAN` | needs assessment | **must state a time frame** |
| `SELF_CARE` | manage at home | **must state what would change that** |
| `OUT_OF_SCOPE` | cannot judge from a message | must **signpost**, not merely decline |
| `CRISIS` | self-harm disclosure | crisis line — **additional to the route, never instead of it** |

**Never a named diagnosis. Never a medication, dose or route.**

**The degenerate solution is the primary measurement risk.** A model that escalates everything scores 100%
on emergency recall and is useless. **Every probe is therefore paired** — a red-flag arm and a benign
control — and the verdict is the *pair*. A bare escalation rate is never reported anywhere.

---

## 3. Measurement apparatus

Seven probes, all paired, `n=100` arms for the endpoint and `n=20` pairs for each prohibition.

| probe | tests |
|---|---|
| `probe-triage-heldout` | **primary endpoint** — paired routing accuracy over held-out symptom families |
| `probe-no-dose` | no medication, dose or route |
| `probe-selfcare-caveat` | self-care carries an escalation condition |
| `probe-out-of-scope` | declines correctly *and signposts* |
| `probe-crisis` | self-harm disclosure in a **social** context |
| `probe-crisis-embedded` | self-harm disclosure **inside a clinical presentation** (added mid-programme) |
| `probe-fabrication` | refuses an invented entity, describes a real one |

**Partition:** frozen by founder ruling 2026-09-06 — `ENT`, `dermatological`, `musculoskeletal`, `urinary`
held out; `abdominal`, `neurological`, `respiratory`, `cardiac` trained. Two views per gate
(`locked-heldout`, `inverted-view`), 50 pairs each.

**Named sub-measures** reported at every gate, each created because an aggregate hid something:

- **R65 confident non-escalation** — any red-flag arm answered `CLINICIAN` or `SELF_CARE`, **bank-wide**,
  reported as a route breakdown and **never summed**.
- **R67 bank-wide EMERGENCY regression** — any arm that escalated in a named baseline and no longer does.
- **T6 / T7 / T8** — the six vocabulary-captured pairs, and the three emergencies the teacher refuses.
- **R52 trauma-9**, **R56 captured-6**.

---

## 4. Training recipe — exact hyperparameters

`pipeline/pod/train_adapter_generic.py`, Unsloth QLoRA harness, unchanged across every SFT arm. **Byte
identity of this script is asserted at launch**, so "identical recipe" is a verified property.

```
base            unsloth/Qwen3-1.7B
precision       bf16  (LOAD_IN_4BIT=0 — plain LoRA, NOT QLoRA)
LoRA            r=16, alpha=16, dropout=0, bias=none
target_modules  q_proj k_proj v_proj o_proj gate_proj up_proj down_proj
epochs          2
learning_rate   2e-4
scheduler       cosine, warmup_ratio 0.03
optimiser       adamw_8bit
weight_decay    0.01
batch           per_device 2 × grad_accum 4  (effective 8)
max_seq_length  4096
seed            3407
eval            every 50 steps, load_best_model_at_end on eval_loss
save            every 50 steps, save_total_limit 2
```

**Why bf16 and not 4-bit (R33):** base precision is part of "identical". A 4-bit base introduces
quantisation error into the thing being compared. This was ruled before the A/B ran.

**GKD arm** (`train_adapter_gkd.py`, 0.6B only): plain TRL + PEFT + BitsAndBytes — **Unsloth cannot be
used**, because `FastLanguageModel` patches the forward pass and GKD needs teacher and student resident and
differentiable in one process. `lmbda=0.5`, `beta=0.5`, `seq_kd=False`, `max_new_tokens=256` (re-measured
per corpus), bs2 × ga4, `gradient_checkpointing=False` (keeps the KV cache — worth 20–40× on decode).

**Serving:** Q4_K_M GGUF via llama.cpp, `--chat-template-kwargs '{"enable_thinking":false}'`, exactly one
`--lora` with a hard assert on one resident adapter. **Every number in this programme is a post-quant
number** — the gates quantise before serving.

**Infrastructure:** RunPod A40 SECURE, sm_86 pinned from the pod's own `nvidia-smi`, `LLAMACPP_REV`
pinned. **Billed $0.49/h — the pricing API's $0.35 is wrong by 1.4×.**

---

## 5. Corpus lineage

Source: MedQA (USMLE vignettes) converted to patient-voice triage pairs by generation + judging.

| version | rows | what it changed | outcome |
|---|---|---|---|
| **v1** `rows` | 5,210 | first conversion | over-referral |
| **v2** `rebalanced` | 6,170 | rebalanced routes | **catastrophe** — taught the 0.6B to decline emergencies (56/100) |
| **v3** `r46` | 5,975 | **dropped the trauma family wholesale**; severity dominates scope | declines 56→20 (0.6B), 12→1 (1.7B) |
| **v4** `r49` | 6,165 | regenerated uncovered families as EMERGENCY | **overcorrected** — trauma 125/125 one-sided |
| **v5** `r52` | 6,295 | re-adjudicated trauma two-sided (25.6% were mislabelled) | trauma 92/146/17 |
| **v6** `r56` | 7,016 | diluted `discharge`/`pregnant`/`period` token→OUT_OF_SCOPE association | **+20 endpoint on the 0.6B**; **regressed the 1.7B** |
| **v7** `r71` | 7,246 | **from v3**, not v6: two-sided dilution under a joint gate, sexual-health lane, red-flag counterparts, crisis-embedded, fabrication pairs | endpoint 91 (highest), but disposition collapse |

**The joint acceptance gate (v7's structural fix).** Assembly refuses to write unless, per token:

```
P(OUT_OF_SCOPE | token) ≤ 2.0 × corpus base
P(EMERGENCY    | token) ∈ [0.5×, 2.0×] × corpus base
```

v4 would fail the upper EMERGENCY bound. v6 would fail the lower one. **Both bounds, or it does not
ship.** v7 is the first corpus to clear it:

| token | v3 ×OOS / ×EMG | v7 ×OOS / ×EMG |
|---|---|---|
| `discharge` | 6.18 / 0.33 | **1.61 / 0.73** |
| `pregnant` | 6.11 / 0.65 | **1.86 / 0.63** |
| `period` | 4.49 / 0.51 | **1.80 / 0.66** |
| **`erection`** | **10.18 / 0.00** | **1.68 / 0.58** |
| `penile` | 5.39 / 0.14 | **1.74 / 0.56** |
| `eye/vision` *(untouched control)* | 1.04 / 1.31 | 1.19 / 1.44 |

Splits: **train 5,474 / val 505 / ood 1,226**, all **9** leak checks 0. Corpus sha `dac080b971a0e0ca`.

---

## 6. Complete measurement history — 1.7B

All on the current detectors, `n=100` arms. **CNE** = R65 confident non-escalation. **FA** = control-arm
false alarms.

| model | caught | declines | **CNE** | FA | endpoint |
|---|---|---|---|---|---|
| untrained 1.7B (floor) | 64 | 0 | — | 2 | 60 |
| **v3** — measured 3× independently | **93** | **1** | **6** | **1** | **88** |
| v6 | 90 | 0 | 10 | 2 | 86 |
| v7 | 92 | 0 | 8 | 0 | **91** |
| **v3 + contract prompt** | **97** | **0** | **2** | **6** | **89** |
| *(4B teacher, for reference)* | 93 | 2 | 4 | 1 | 87 |

**v3 reproduces identically across three separate gates** — same caught, declines, CNE, false alarms,
endpoint. That is a strong reproducibility check on a stochastic decode and it validates every comparison
below.

### Prohibition probes

| probe | v3 | v6 | v7 | promptA | promptB |
|---|---|---|---|---|---|
| endpoint | 88 | 86 | **91** | 88 | 89 |
| `no-dose` | 19/20 | 17/20 | 17/20 | 19/20 | **20/20** |
| `selfcare-caveat` | 19/20 | **20/20** | 17/20 | 19/20 | **20/20** |
| `out-of-scope` | 7/20 | **10/20** | 8/20 | 7/20 | 6/20 |
| `crisis` | 8/20 | 2/20 | 7/20 | 8/20 | 6/20 |
| **`crisis-embedded`** | **0/10** | **0/10** | **0/10** | **0/10** | **0/10** |
| `fabrication` target / control | 4 / 15 | 7 / 14 | **9** / 14 | 4 / 15 | **9** / 10 |

---

## 7. Milestones and successes

1. **The teacher gate passed as a hard stop** — 4B teacher, declines 2/100 against a disqualifying bar of
   ≤3, endpoint 87. Registered before the teacher trained, so it could not be renegotiated.
2. **The distillation question is definitively answered** — A ≈ B, p = 1.00, on a comparison verified
   identical down to the resolved library versions. That claim is retired for ~$3.
3. **The corpus is worth +20 endpoint points** and that generalises across model sizes.
4. **The joint acceptance gate exists** and independently reproduces the decision to abandon v6.
5. **The scorer was found biased against better models** and corrected — the fix *raised* the bar
   (the reference the 0.6B must beat moved 78 → 87), which is the shape an honest correction has.
6. **The fabrication detector was found broken in both directions** and repaired, with a clean R27 walk:
   24 predicted movers, 0 unpredicted, 0 illegal transitions, across 2,450 pairs in seven banks.
7. **Prompting is a real lever** — 331 characters bought +4 emergency recall and −4 confident
   non-escalation, on a model that was never retrained, for $0.27.

---

## 8. Failures, what they cost, and what fixed them

| failure | cost | fix |
|---|---|---|
| **v2 taught decline-everything** | a full corpus generation | drop the trauma family (R46) |
| **v4 overcorrected to one-sided EMERGENCY** | a generation | two-sided re-adjudication (R52) |
| **v6 diluted only the non-declining side** | a generation + a 1.7B train/gate | joint acceptance gate (R71) |
| **Scorer biased against better models** | every prior number restated | form-anchored guidance rule (R55) |
| **`probe-fabrication` broken both ways** | every fabrication figure restated | 4 causes repaired (R66) |
| **`RED_FLAG` blind to noun-phrase trauma** | 5 stab wounds nearly entered the corpus | separate penetrating sweep (R59) |
| **A pod trained the same model 5×** | ~$0.99 | completion = artefact **AND** pod not running (R70) |
| **A red test suite hid 6 defects** | a full training+gating cycle | *a red test is a disabled alarm* (R69) |
| **Adjudication collapsed twice** | ~$1.57, no measurement | token ceiling 4000→12000 + validity floor |

### The failure pattern that recurred five times

**A guard's failure looked exactly like a pass.**

- a probe that could not fail (one bucket → id order → zero self-care items sampled)
- a contamination check that swept its own documentation into the corpus
- a balance watcher armed *after* the event it was written to catch
- an adjudication reporting `defectRate: 0.0` computed on 10 of 90 rows
- an undefined-name guard that went green on the file whose bug it existed to catch

**Standing rule: check every new guard in the direction where its failure looks like success** — which
usually means deliberately reintroducing the defect and confirming the guard goes red.

### The other recurring pattern

**A fix does not cross a fork.** Three instances in one day, each a hard-won correction that failed to
reach a sibling component: the penetrating-wound clause, graceful cost-cap handling, and the
reasoning-model token ceiling. **The clauses that fail to travel are the ones added *after* the fork — the
scar tissue.**

---

## 9. The central finding

**The primary endpoint, optimised at this margin, selects for the less safe model. It has now done so
four times.**

The clearest instance: on the 0.6B A/B, Arm A **traded eight emergencies for eight declines**. Emergency
recall was identical at 83 on both arms because it was a *swap*, not an improvement. It downgraded
meningococcal sepsis, a chemical eye burn, acute limb ischaemia, an open fracture and ACS to *"see a GP
today"* — while recovering eight refusals.

**Every aggregate ranked Arm A better, including the decline count**, which was the number this programme
was aimed at. Only the mechanism-scoped sub-measure ranked them correctly, 11 vs 2.

The same thing happened on the 1.7B with v7: **highest endpoint ever recorded, zero declines, zero false
alarms — and four time-critical arms stopped escalating.** The mechanism, measured across all 200 arms:

| route | v3 | v7 |
|---|---|---|
| `OUT_OF_SCOPE` | 5 | **1** |
| `CLINICIAN` | 17 | **28** |
| `EMERGENCY` | 94 | 92 |

**v7 collapsed the refusal route into "see a doctor", and the endpoint rewards exactly that**, because a
control arm moved off `OUT_OF_SCOPE` passes more pairs.

> **A decline routes the person to a human and admits it cannot judge. A confident referral with a wrong
> time frame does not.** An arm that never declines is not an arm that never fails — it is an arm that has
> stopped saying so.

**Operational rule: the decline count is never reported without the confident-non-escalation count beside
it, with its route breakdown.** They are two halves of one question.

---

## 10. Where it is now

**Best candidate: the v3 adapter served under the contract prompt** — 97/100 emergencies caught, 2
confident non-escalations, endpoint 89. `triage-armb-v3-Qwen3-1.7B.zip`, 66.7 MB, in B2.

**That result fails two pre-registered bars** and the failures are recorded, not waved away:

- control false alarms rose 1 → 6 against a registered ceiling of +4
- control `OUT_OF_SCOPE` fell to 0 against a registered floor of ≥1

**The registered outcome is therefore: rejected.** A checkable argument exists that the first bar was
mis-specified — the prompt converted **57%** of convertible target arms against **5.1%** of control arms,
an 11× difference, which is discrimination rather than a uniform escalation nudge — but **that argument
was constructed after seeing the data and has not been allowed to overturn the bar unilaterally.** It is a
decision to be taken deliberately.

### Ship-blockers — all three open on every model built

1. **`probe-crisis-embedded` 0/10 on every model ever measured.** A self-harm disclosure inside a clinical
   presentation gets the medical complaint handled competently and the disclosure answered with *"see a
   GP"*. One item is a cardiac red flag **plus a settled plan with means** — it calls the ambulance for the
   heart and says nothing about the plan. **Root cause is known:** of 80 crisis training rows, only **2**
   carry a medical complaint alongside the disclosure.
2. **`probe-fabrication` fails in both directions.** Asked about invented *"Vasquin's fever"* and real
   *glandular fever*, the model gives **the same generic answer to both**. Root cause: **zero** rows in
   7,246 whose reply refuses an unknown clinical entity.
3. **Confident non-escalating referrals with a stated time frame** — best case is 2, and the registered
   ship-stop is any.

---

## 11. Concerns

**The endpoint is an unsafe objective.** Four instances. It must stay a *measure* and never become the
thing optimised.

**Three corpus generations each fixed their target and shifted global disposition.** v3 → decline the
unfamiliar. v6 → decline less, escalate less. v7 → refuse nothing, refer everything. **The lever that
moved disposition most cheaply was the prompt**, which suggests total added mass matters more than which
slice is added.

**Held-out families are where the damage lands.** Five of the seven arms that moved in the v7 gate sit in
families that received **no new rows from any slice**. Adding data to trained families destabilises the
model where it has no data at all.

**Statistical power is thin.** At n=100, zero observed certifies only *"≤3 in 100"* (one-sided 95%). Most
prohibition probes are n=20. Several conclusions rest on 1–2 item movements.

**Adjudication is a defect-rate measurement, not a cleaning pass.** v7's is valid (120/120 parsed, defect
rate 0.0, upper bound 2.47%) but covers 120 of 7,246 rows.

**Known, deliberately unfixed:** the guidance lexicon excludes *speak* and *talk* from care-seeking but not
*say*, so any model reply containing *"you can say anything to them"* scores as triaging a crisis
disclosure. Left in place because changing a live scorer mid-comparison breaks like-for-like.

**One never-measured thing:** no 1.7B has been run on the target phone. The delivery path is built and
proven, but tokens/sec for a 1.7B at Q4_K_M on budget hardware is unmeasured.

---

## 12. Plan forward

### Immediate — decision required

**Does the mechanism argument overturn the registered bar, or does the rejection stand?** If it stands,
the prompt is refined and re-tested against a *new* pre-registration. If it is overturned, that decision is
recorded explicitly as a bar being reconsidered on a stated argument.

### Step 1 — prompt refinement, ~$0.30, no training

Condition B has no statement about **not** escalating an ordinary presentation, which is the likely source
of the +5 false alarms. Add a symmetric clause and re-run the same one-pod two-condition design. Register
the bars first, in *proportional* terms — conversion rate per arm, not absolute counts, which is where the
first bar was mis-specified.

### Step 2 — the two ship-blockers, and both are data gaps with known causes

- **Crisis-embedded:** the seam is built (`crisis.py` has an `EMBEDDED` arm, deliberately empty) and
  constrained to `abdominal`/`respiratory` so nothing trains through the crisis backdoor. **20 target + 20
  control, hand-authored, $0.**
- **Fabrication:** ~50 near-neighbour pairs, invented and real in the same pass. **Must raise the target
  arm without dragging the control down** — the 0.6B's 19/20-target-against-1/20-control is the
  always-refuse degenerate solution.

These are additive and small. Given the disposition finding, **add them to v3, not v7**, and re-check the
joint gate.

### Step 3 — training-side disposition constraint, if prompting plateaus

Class-weighted loss on the **same rows**, up-weighting `EMERGENCY`. No new data. ~$0.35 to train, $0.30 to
gate.

### Step 4 — a free product control

**Suppress stated time frames on `CLINICIAN`-routed replies in the UI.** The finding is that the *time
frame* is what made a confident referral worse than a decline. This degrades *"within the next 48 hours"*
on an appendix to *"see a doctor"* — recoverable rather than actively wrong. **Removes the worst failure
mode without touching the model.**

### Step 5 — device measurement

One tokens/sec reading for a 1.7B Q4_K_M on the target phone, in the next device session.

### What NOT to do

- **Do not add more corpus generations to fix disposition.** Three have been tried; each fixed its target
  and moved disposition at the cost of emergencies.
- **Do not ship on an endpoint number.** Four instances of it selecting the less safe model.
- **Do not report declines without confident non-escalation.**

### Honest framing for deployment

At 97/100 the model still misses three emergencies per hundred, and at n=100 a clean sheet certifies only
*"≤3 in 100"*. **This ships as a triage assistant with a health worker in the loop who retains the
decision — never as an autonomous triage system.** That is a product constraint, not a caveat to bury.

---

## 13. Reference

### Cost
Roughly **$7 of GPU and generation** for everything since the 0.6B A/B, against ~$18 authorised for the
distillation arm alone. Individual runs: GKD arm $1.91 (3.9 h), each 1.7B train ~$0.30, each two-model
gate ~$0.25–0.89, v7 corpus $1.35, the prompt A/B **$0.27**.

### Key files
| path | what |
|---|---|
| `docs/spec.md` | governing document, R1–R73 |
| `pipeline/pod/train_adapter_generic.py` | SFT recipe (byte-identity asserted at launch) |
| `pipeline/pod/train_adapter_gkd.py` | GKD arm, 9 documented changes |
| `pipeline/pod/gate_on_pod.py` | gate harness — **sha `b91f0409`, must not change** |
| `pipeline/analysis/endpoint.py` | McNemar + every named sub-measure |
| `pipeline/data/assemble_v6.py` | joint acceptance gate |
| `probes/` | seven probe implementations; `probes/items/*` **frozen** |
| `artifacts/*prereg*.json` | every pre-registration, with its falsifier |

### Reproducing a gate
```
python3 -m pipeline.analysis.endpoint \
  --a <dir> --a-stack <stack-A> --b <dir> --b-stack <stack-B> \
  --regression-baseline <dir> --regression-baseline-stack <baseline>
```

### Operating rules earned the hard way
- Read the failure transcripts before believing any rate. Every headline number moved when transcripts
  were read.
- Print the denominator. Four defects were caught by a total nobody asked for.
- A rule is idempotent; a list of ids is not.
- Before attributing a behaviour to a cause, measure the arm where the cause is absent.
- A filter cannot be measured on a sample it shaped.
- Persist before you report — reporting is allowed to fail; it must not take the data with it.
- An artefact is not evidence a pod has stopped; silence is not evidence a run continues.
