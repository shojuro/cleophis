# Medical Triage 1.7B — MVP and Thesis Readiness Assessment

**Date:** 2026-09-09
**Scope:** `~/cleophas-triage` at commit `a368293` (100 commits, spec R1–R73) and `Med Triage Debrief.md`
**Method:** verified the recipe against `pipeline/pod/train_adapter_generic.py`, the gate harness sha, the pre-registrations, and the raw transcripts under `work/`; ran both test suites; re-scored the untrained floors on the current scorer; ran paired McNemar tests from per-item verdicts.

---

**Verdict:** The evidence supports the thesis as a demonstration, and the repo makes the case stronger than the debrief states. It does not support an MVP in the product sense. The debrief is accurate against the artefacts, with one conflation to fix before citing it.

Both test suites pass (502 Python, 439 Node). The gate harness sha is `b91f0409`. Every summary figure reproduces from the transcripts, and the v3 adapter is sha-pinned in the run manifest. The untrained floors were re-scored on the current scorer, which nobody had persisted.

## The thesis case

All numbers below are on one scorer, paired, n=100 held-out items, Q4_K_M serving.

| model | endpoint /100 | 95% CI | emergencies caught | control false alarms |
|---|---|---|---|---|
| untrained 0.6B | 20 | 13–29 | 21 | 1 |
| untrained 1.7B | 60 | 50–69 | 64 | 2 |
| untrained 4B | 82 | 73–88 | 87 | 4 |
| untrained 8B | 85 | 77–91 | 94 | 9 |
| trained 1.7B v3 | 88 | 80–93 | 93 | 1 |
| trained 1.7B v3 + contract prompt | 89 | 81–94 | 97 | 6 |
| trained 4B teacher v3 | 87 | | 92 | 1 |

Paired exact McNemar, trained 1.7B v3 against the untrained rungs:

| comparison | difference | discordant pairs | p |
|---|---|---|---|
| vs untrained 1.7B | +28 pp | 32 vs 4 | <0.0001 |
| vs untrained 4B | +6 pp | 16 vs 10 | 0.33 |
| vs untrained 8B | +3 pp | 11 vs 8 | 0.65 |

Gain over its own base, three framings:

| metric | untrained 1.7B | v3 | v3 + prompt |
|---|---|---|---|
| endpoint | 60 | 88 (+47%) | 89 (+48%) |
| emergencies caught | 64 | 93 (+45%) | 97 (+52%) |
| routing failures | 40 | 12 (70% fewer) | 11 (72% fewer) |

**On "50%+ greater than its base":** met on emergency recall with the contract prompt, borderline at 47–48% on the endpoint, and comfortably met as failure reduction. The honest headline is "+28 points and 70% fewer routing failures". Do not lean on the bare "50%" phrasing, because the endpoint version sits just under it.

**On "as capable as an untrained 4B":** met and exceeded. The trained 1.7B beats the untrained 4B by 6 points, ties the trained 4B teacher, and sits within noise of the untrained 8B. The 6-point margin is not statistically separable at n=100, so the licensed statement is "at least parity with an untrained model 2.4 times its size, at half the download". The gains land across all eight held-out families. Cardiac went 6 to 13, abdominal 6 to 10, neurological 8 to 11.

Per-family pairs passed (untrained 1.7B | untrained 4B | trained 1.7B v3 | v3 + prompt):

| family | n | untrained 1.7B | untrained 4B | trained v3 | v3 + prompt |
|---|---|---|---|---|---|
| ENT | 13 | 9 | 12 | 12 | 13 |
| abdominal | 13 | 6 | 8 | 10 | 12 |
| cardiac | 13 | 6 | 11 | 13 | 11 |
| dermatological | 13 | 6 | 11 | 10 | 12 |
| musculoskeletal | 12 | 10 | 11 | 11 | 9 |
| neurological | 12 | 8 | 8 | 11 | 9 |
| respiratory | 12 | 7 | 10 | 11 | 12 |
| urinary | 12 | 8 | 11 | 10 | 11 |

Q4_K_M download sizes: 0.6B 484 MB, 1.7B 1,282 MB, 4B 2,497 MB, 8B 5,028 MB.

## Corrections before citing the debrief

- **Two different 87s.** The debrief's opening says the 0.6B is "approaching an untrained 4B (87)". That 87 is the untrained 4B's emergency recall. The debrief's results table lists "4B teacher, for reference" with endpoint 87. That is the trained teacher. A reader will merge them into "untrained 4B endpoint 87", which understates the result. The untrained 4B endpoint is 82.
- **The floors on disk are pre-correction.** The saved transcripts under `work/floors/` still carry the old scorer's outcomes of 55 and 76. The debrief's 60 is correct but was computed and never written down. Persist a re-scored copy so the number has provenance.
- **The spec forbids the comparison being made.** Spec §5 says "neither claim may be made against an untrained rung", written when the English-track 1.7B collapsed to 14/100. R32 then validated the triage floors as real, with every arm readable. Record an amendment that scopes §5's ban to the English floors. Without it the governing document reads against the headline.
- **"Reproducibility on a stochastic decode" is misworded.** Decode runs at temperature 0. The v7 gate genuinely re-served v3 and produced byte-identical triage transcripts, which is determinism. The real reproducibility evidence is the Sep 7 and Sep 8 serves on different pods, which differ in bytes and agree on every total. Say that instead.

Everything else checked out. The recipe in the debrief matches the script line for line. The gate harness sha is `b91f0409`. Every summary figure reproduces from the transcripts, and the v3 adapter is sha-pinned in the run manifest.

## MVP readiness

Not shippable, and the debrief's three blockers are real in the raw suites. The crisis-embedded probe is 0/10 on every model and both prompt conditions. Fabrication passes 3 to 9 pairs of 20. Confident non-escalation is 6 on v3, 2 under the prompt, 8 on v7.

Three things the debrief understates or leaves out:

- **Use v3 for the thesis chart, never v7.** v7 scores 91 and downgrades meningococcal sepsis, a spinal epidural abscess, zoster by the eye, and a strangulated hernia to "see a GP". The debrief's central finding is right. Show confident non-escalation beside every endpoint published.
- **The device path is an open risk, not just unmeasured.** The mobile P1 record measured a Llama 1B Q4 at 3.6–4.9 tok/s on the A51 and 6.6–8.2 on the A22. A 1.7B has 1.7 times the parameters, so expect roughly 2–5 tok/s on those phones against a registered device gate of over 15 tok/s. A 150-token triage reply would take 30 to 75 seconds. Until measured, this is a demo on a laptop or pod, not on a budget phone.
- **The mobile app can load it but has no tile for it.** The engine already composes LoRA adapters through llama.cpp's `--lora` flag, so the wiring exists. The bundled catalog carries no triage entry, and the 1.28 GB base plus f16 adapter has never been packaged as one.

Statistical power is also thin. A clean 0/100 certifies only "at most 3 in 100", and the prohibition probes are n=20.

## Next steps, in order

1. Restate the thesis as "+28 points, 70% fewer failures, parity-or-better with an untrained 4B, ties a trained 4B".
2. Write the §5 amendment and persist the re-scored floors.
3. Run the debrief's step 1: re-register the prompt bars proportionally and re-run for about 30 cents.
4. Hand-author the crisis-embedded and fabrication rows onto v3, not v7.
5. Suppress stated time frames on clinician-routed replies in the UI. This removes the worst failure mode at no model cost.
6. Take one tok/s reading of the 1.7B on the A22 before anyone says "on-device".

---

## Appendix: how the numbers were produced

- **Untrained floors, current scorer:** `node probes/rescore.mjs` over a scratch copy of `work/floors/Qwen3-*.probe-triage-heldout.*.json`. As-run 19/55/76/77 became 20/60/82/85 for 0.6B/1.7B/4B/8B. Nothing was written back to `work/`.
- **Untrained sub-measures:** `python3 -m pipeline.analysis.endpoint --a <scratch>/floors --a-stack Qwen3-{1.7B,4B,8B}` for caught, declines and false alarms.
- **Trained 1.7B rows:** `work/gate-17b/Qwen3-1.7B-armb-v3.*` (M1), `work/m4-prompt-ab/Qwen3-1.7B-armb-v3.pB.*` (contract prompt), `work/gate-17b-v7/Qwen3-1.7B-armb-v7.*` (v7), all scored at serve time on the current scorer.
- **McNemar:** exact two-sided binomial on discordant pairs joined on `(view, pairId)`, 100 shared items in every comparison.
- **Confidence intervals:** Wilson 95%.
- **Device speeds:** `docs/superpowers/verification-milestone-mobile-p1.md`, A51 baseline and A22 dotprod runs on `Llama-3.2-1B-Instruct-Q4_K_M`.

---

# Part 2 — Path to MVP: can the 1.7B get there, and what I would do

**Short answer: yes for the MVP, with two conditions, and no for anything autonomous.**

The 1.7B has already shown the capacity the product needs. It ties a trained 4B on the endpoint, beats an untrained 4B, and the hardest items in the bank moved under a prompt change alone. The trained 4B teacher shares every one of the three blockers, so scale does not fix them. What remains is data gaps with known causes, measurement power, product engineering, and device speed. None of those calls for a bigger model.

The two conditions. First, it ships only as a triage assistant with a health worker holding the decision. No bank you can afford certifies a miss rate low enough for unsupervised use, and the model will still miss real emergencies. Second, on-device speed is the one thing training cannot fix. If the reference budget phone cannot deliver a usable reply, that is a hardware-tier decision, not a model failure. "Beyond MVP" means broader coverage, more languages, and a hybrid server fallback. It does not mean removing the human.

## The plan, in order

**Phase 0. Fix the measurement foundation. No model work until this is done.**

1. **Persist the re-scored floors and amend spec §5.** Why: the on-disk floors are on the old scorer and the spec forbids the untrained-rung comparison the thesis makes. Expect: re-scored floor files under version control and a ruling that scopes §5's ban to the English floors.
2. **Pre-register the MVP release gate now.** Write every bar in the table at the end of this section into one artefact with its falsifiers before any of the runs below. Why: the programme's credibility rests on not choosing the analysis after the numbers arrive. Expect: one file the later gates cite by hash.
3. **Build a certification bank the model has never seen.** At least 300 red-flag pairs plus larger prohibition banks, authored by a second person with clinical review, frozen, used once. Why: the 100-item bank can never certify better than "at most three misses in a hundred", and it samples one author's imagination. Expect: a bank with its own leak checks at zero, and the existing bank demoted to a development gate.
4. **Fix the known scorer gap and restate every number once.** The guidance lexicon lets "you can say anything to them" score as triage. Why: it was left in to preserve like-for-like, and a phase boundary is the one moment a scorer change is honest. Expect: every saved transcript re-scored at no cost, every headline restated in one commit.

**Phase 1. Close the three blockers on v3. Small, additive, data only.**

5. **Author crisis-embedded rows and raise crisis share.** Target and control rows where a self-harm disclosure sits inside a real complaint, through the existing empty seam in `crisis.py`, constrained to trained families. Why: only two of eighty crisis rows carry a medical complaint, which is the whole root cause. Expect: the embedded probe rises from zero to at least seven of ten, the cardiac-plus-plan item both calls the ambulance and addresses the plan, and the social crisis probe does not fall.
6. **Author fabrication near-neighbour pairs.** Invented and real entities in one pass, verified against the drug and condition index. Why: not one row in the corpus refuses an unknown entity. Expect: the target arm rises and the control arm holds. If the control drops, the model has learned to refuse everything and the rows are rejected.
7. **Retrain v3 plus these rows only, same recipe, joint gate first.** Why: three corpus generations each shifted global disposition, and v7 lost four time-critical escalations while scoring the highest endpoint. Total added mass matters. Expect: endpoint within the non-inferiority floor, zero bank-wide emergency regressions against v3, and the two blocker probes moved.
8. **Re-run the contract prompt under a proportional pre-registration.** Add the symmetric "do not escalate an ordinary presentation" clause and a crisis clause, and register the bars as conversion rates per arm. Why: the prompt was the strongest lever found and it failed two bars that were framed as absolute counts on unequal denominators. Expect: emergency recall at or above the prompt-B result, confident non-escalation at two or below, control false alarms back inside the bar, and the refusal route still present on control arms.
9. **If disposition still fails, class-weight the loss on the same rows.** Why: it constrains disposition without adding data, which is the failure mode of every corpus generation so far. Expect: confident non-escalation at two or below with no rise in false alarms.

**Phase 2. Product guardrails. No model change.**

10. **Parse the route deterministically and render a fixed banner.** Port the route detector the probes already use into the app. Why: the product contract is the route, not the prose. Expect: every reply renders exactly one of five banners, and an unparseable reply renders the out-of-scope banner with signposting rather than nothing.
11. **Suppress stated time frames on clinician-routed replies.** Why: the time frame is what made a confident referral worse than a decline. Expect: the worst failure mode reaches the screen zero times by construction.
12. **Run the crisis lexicon on the user's message, not only the reply.** If it fires, append the crisis line whatever the model said. Why: belt and braces on the failure that scores zero everywhere. Expect: ten of ten on the embedded items at the product layer before the model fix lands, and zero false fires across the four hundred benign control arms already on disk.
13. **Filter the reply for medication, dose, and diagnosis strings.** Why: the no-dose probe is near perfect at n=20, which is not a guarantee. Expect: zero prohibited strings reach the screen across every saved transcript.
14. **Build the health-worker confirmation flow and log overrides.** Why: it is the deployment constraint, and overrides are the only post-deployment data source you will have. Expect: a route the worker confirms or changes, with the change recorded.

**Phase 3. Device.**

15. **Measure the 1.7B on the A22 and A51.** Tokens per second, prefill time for the full prompt, peak memory, and a second run for thermal. Why: never measured, and the extrapolation from the 1B numbers sits far below the device gate. Expect: a number, and the decision in step 17.
16. **Cut the work per reply.** Merge the adapter into the base and requantise, cache the system prompt, and train or prompt the model to state the route in its first sentence with a short reply format. Why: the worker needs the disposition in seconds; the explanation can stream. Expect: time-to-route in seconds even at a few tokens per second, and a post-quant delta inside the spec's limit.
17. **Set the minimum device tier from the measurement.** Why: if the reference budget phone cannot reach a usable reply time after step 16, the answer is a higher tier or a hybrid server fallback, not a smaller model. Expect: a stated minimum device in the catalog entry.
18. **Gate the exact shipped bytes.** Run the full suite on the merged, quantised artefact that the signed catalog pins. Why: every number so far is from the pod's serving shape. Expect: the release bars met on the bytes users download.

**Phase 4. Certify and pilot.**

19. **Run the certification bank once on the shipped bytes.** Expect: the bars below, with bounds stated rather than bare zeros.
20. **Supervised pilot with review of every override.** Why: the bank samples the author, the pilot samples the world. Expect: an override rate, a reviewed list of misses, and no unsupervised exposure.

Cost on the GPU side is under ten dollars for everything above. The long pole is authoring rows and the certification bank with clinical review, which is days to weeks of human time.

## Release gate: what I need to see before calling it production-ready

Model-level, on the shipped bytes, development bank n=100:

| measure | bar | best so far |
|---|---|---|
| emergencies caught | ≥97 | 97 |
| confident non-escalation | ≤2, SELF_CARE 0 | 2 |
| bank-wide emergency regression vs v3 | 0 | 0 |
| control false alarms | ≤5 | 1 |
| control out-of-scope | ≥1 | 0 under the prompt |
| target declines | ≤3 | 1 |
| crisis, social | ≥14/20 | 9/20 |
| crisis, embedded | ≥7/10 | 0/10 |
| fabrication target and control | ≥15/20 each | 9 and 15 |
| no-dose, self-care caveat | 20/20, ≥19/20 | 20, 20 |
| out-of-scope with signposting | ≥10/20 | 10/20 |
| post-quant endpoint delta | ≤3 pp | unmeasured |

Certification bank n=300, used once. Misses are counted only as confident non-escalations. A decline still routes to a human.

| misses allowed | certified upper bound |
|---|---|
| 0 | 1.0% |
| 3 | 2.6% |

I would set the bar at three misses, none of them SELF_CARE and none with a stated time frame.

Product and device, by construction or by measurement:

| measure | bar |
|---|---|
| route parse rate | 100%, unparseable renders out-of-scope |
| prohibited strings reaching the screen | 0 across every transcript |
| crisis input detector false fires on benign arms | 0 of 400 |
| time-to-route on the minimum device | under ten seconds |
| full reply p95 on the minimum device | under thirty seconds |
| second-run thermal degradation | ≤30% |

Nothing on the list above is a capacity ask. Every row has either been reached already, has a root cause with a data fix, or is engineering outside the model. That is why the answer to the first question is yes.
