# Phase 1c — release-gate instruments, the M7 round (v10 / v10d), GRPO in reserve

## Context

Phase 1b (crisis grafting + DPO; triage spec R78) closed with NOTHING SHIPS at $2.37. Its reading, the
whole-branch review, and a plan-review stress test (`.superpowers/sdd/2026-09-10-med-triage-phase1b-grafting-dpo/p1c-plan-review.md`, two cross-tabs + two corpus measurements) establish:

- **What v9 did.** Displacement went 2 → 0, false alarms 7 → 1, control over-signposting 10 → 6. But v9
  learned a **scope-disclaimer refusal** on acute presentations ("I cannot judge how serious this is — see a
  GP"), which `detect_route` ranks above a plain CLINICIAN branch. Disclaimer prevalence over the 420 gate
  replies rose 34 → 89 and it is NOT distress-conditioned (46 of the 55 extra land on banks with no
  distress). It explains 8 of 9 `no_dose` failures and 10 of 13 lost `caught` arms: `declines` 0 → 10,
  `caught` 100 → 87, `no_dose` 17 → 11, endpoint 85 → 69. No corpus QUANTITY explains the rise (weighted
  disclaimer mass fell 9.69% → 9.58%); the conditioning changed: the 106 out-of-scope graft rows are the
  only rows where a disclaimer answers an urgent-sounding turn.
- **Omission** (the crisis line missing: 51% of greedy target replies) is terminal-slot dropout on early
  EOS: EMERGENCY targets were never shown a non-terminal crisis line (`supportMidByRoute.EMERGENCY: 0`).
- **The embedded bar's arithmetic.** Three of the ten pairs are SELF_CARE, two of them ENT (held out; A28
  forbids grafting there); no arm has ever passed any of the three (their controls escalate under
  distress). 7 of 10 needs two of those three plus a near-perfect EMERGENCY/CLINICIAN half (best ever
  3 of 7). Honest odds: the M7 data fix restores `caught`/`no_dose` ~60%; M7 reaches 7/10 ~10%; one GRPO
  round ~12% (~20% post-guard); every disqualifying bar at once ~4% (`crisis ≥ 14` and `no_dose ≥ 20`
  are strictly anti-correlated across all five gated arms). The product guard (`src/triage/guard.js`
  `applyGuard`) recovers omission but not displacement or scope refusal, and bought zero embedded pairs
  on v9 because control drift binds.
- **v9d** (DPO, two named adapters) moved behaviour the right way and answered the worst embedded item
  correctly but is ineligible by its post-training reference-drift guard (mechanism unresolved; bf16
  run-to-run nondeterminism is the leading hypothesis, ~70% that v9d was in fact fine). The trainer's
  eligibility rule is now WEIGHT-based (`caf146b`) but has never been registered.
- **Process gaps.** `post_quant_delta` was never measured anywhere and its metric names a merged arm A15
  deleted; the gate already serves `Q4 base + f16 LoRA`, so reporting "0 by byte identity" would claim a
  comparison nobody ran. `regression` was measured vs v8w while registered vs v3 (v3 not a rung). On the
  phone, the engine cannot pass `enable_thinking` (it uses `llama_chat_apply_template` with no kwargs),
  runs with thinking ON sharing the 320-token budget, strips `<think>` after the fact, and
  `ThinkStripper` has no flush (a reply truncated inside `<think>` returns an empty string).

Founder decisions (2026-09-11): scope = instruments + M7, RL in reserve; `device_delta` release-only,
disqualifying at release; training ceiling $3 for this plan; keep the 7/10 bar registered and decide the
partition/bar question AFTER M7's reading (report embedded in BOTH forms, model-level and post-guard);
add an ADVISORY prompt condition pF (pB minus the fourth-disposition clause) on the M7 gate.

Nothing is deployed. Everything here is pre-deployment measurement and training. Deployment = the pod
release gate passing + the device bars passing + publishing the adapter GGUF to the signed `cleophis-dist`
catalog (`tools/pipeline/publish.py`; the gate pod already produces the f16 LoRA GGUF).

## Preconditions (close Phase 1b first)

1. The pending whole-branch re-review of `sdd/p1b-t7` 07d98ce lands Approved (or one more fix round);
   `mvp/phase1b` fast-forwards to it (suites already green: 519 JS / 1,078 py).
2. Finishing menu for `mvp/phase1` + `mvp/phase1b` → `main` together (founder's choice); the MVP spec's
   A28/A29 edits and the Phase 1b plan are committed in the mobile worktree.
3. Branches: triage `mvp/phase1c` off the merged tip; mobile `mobile/triage-p3` off `mobile/triage-p2`
   (PR #33 → `mobile/p1-alpha` is still open; the triage catalog fields live only on triage-p2).

## Global constraints

- Standing rules unchanged: `pipeline/pod/gate_on_pod.py` b91f0409 FROZEN (new serves go in a sibling
  script); `probes/items/*` frozen; R73 (every bar/prediction registered and committed ALONE before any
  pod; never edited after data); R70 (DRY_RUN → launch → `watch_b2 --pod-name`; completion = artefact AND
  pod gone); A19 debriefs with curves; spec §7 transcripts read; only delete pods you created; no secrets
  (keys via `pipeline/env.py`); never `cd` into the original repo root; never bare `git stash`.
- A28 (no LLM in the data loop; hand-authored rows detector-validated and bank-swept) and A29 (RL/DPO on
  detector-derived rewards, banks held out) bind every data task.
- New standing rules from Phase 1b: any new or changed pod trainer runs the scratch-venv boot smoke
  (`pipeline/pod/tests/smoke_dpo_boot.py` pattern) with the pod's exact pinned stack BEFORE its review;
  `transformers==4.57.1` on every non-Unsloth pod; the harvest and every generation render exactly what the
  gate serves (the pre-closed think block).
- Rendering parity between pod and device is asserted by a sha of the rendered prompt for a fixed message,
  computed on both sides.
- Budget: Track A ≈ $0.05 of pod time (the f16 serve rides the M7 gate pod); Track B ≤ $3, with a kill
  switch after the first $0.83.

## Track A — release-gate instruments (no GPU)

### A1 — `quant_delta` + `artifact_identity` (triage repo)
- `pipeline/pod/quant_delta_on_pod.py` (sibling of the frozen gate; imports its serve/download helpers or
  re-implements them minimally): keep the f16 base GGUF the gate builds and deletes
  (`gate_on_pod.py:238-244`), serve `f16 base + the SAME f16 LoRA` with the same llama-server flags and
  `--chat-template-kwargs '{"enable_thinking":false}'`, run the endpoint suite (100 pairs) under pB for
  every rung, write `endpoint-<rung>-f16-pB.json` beside the gate's outputs. Invoked by the M7 gate
  launch script after the gate's suites on the same pod (~5 cents).
- `pipeline/analysis/release_gate.py`: measures `quant_delta` = endpoint(Q4+LoRA) − endpoint(f16+LoRA),
  item-paired; `artifact_identity` = served Q4 sha == floor sha 25162bff… == the catalog's base sha AND
  the served adapter GGUF sha == the sha the publish step will upload (recorded in the run manifest).
  Tests on fixtures; both NOT MEASURED → FAIL when absent.
- Release prereg amendment (in the M7 prereg, before the pod; the old bar kept in the text as superseded
  with the A15 reason): `post_quant_delta` inapplicable under A15 → replaced by `quant_delta ≥ −3`
  (disqualifying) and `artifact_identity == true` (disqualifying).

### A2 — device rendering parity (mobile repo, `crates/kpack-engine`)
- `src/llama.rs`: after `apply_chat_template(&self.chat_template, &chat, true)`, for the ChatMl/Qwen3
  family append the pre-closed think block (`<think>\n\n</think>\n\n`) so the rendered bytes equal
  Qwen3's Jinja output under `enable_thinking=false`; expose `rendered_prompt_sha()` for the parity test.
- `src/template.rs`: `ThinkStripper::finish()` and a truncated-in-think state that returns the partial
  text with a flag, never an empty string; keep the stripper as a no-op safety net.
- Tests: byte equality against a fixture rendered by the pod's tokenizer (`Qwen/Qwen3-1.7B` chat template,
  `enable_thinking=False`, cached locally — the Task 5 reviewer found it); the truncation case; the
  catalog's `promptFingerprint` 67b7f1633f30 unchanged.

### A3 — device probe harness (mobile repo)
- `crates/kpack-engine/examples/probe.rs`: `--prompts-file <jsonl>` (`{id, suite, system, user}`),
  `--json` output (`{id, text, raw, tokens, ms, prompt_sha}`), temperature 0, `--max-tokens` asserted
  equal to the pod gate's value, never `--greeting` (the pod sends system + user only).
- `docs/superpowers/mobile-tools/run-device-probes.sh`: pushes the base + adapter GGUFs (by sha) and the
  prompt file over adb, runs the harness, pulls the JSON; prints the prompt-sha parity line.
- `probes/device-guard.mjs` (mobile): applies `applyGuard` from `src/triage/guard.js` to every device
  reply and writes the post-guard text beside the raw text — the display the patient reads.

### A4 — `device_delta` measure + bars (triage repo)
- `pipeline/analysis/device_delta.py`: converts the device JSON into the probe transcript shape
  (`{at, user, text, reasoning, state}`) per suite; pairs each item with the pod gate's transcript for the
  same rung under pB; computes (i) the per-item ROUTE disagreement count with a Wilson 95% interval, raw
  and post-guard, and (ii) the endpoint delta. Registered in the release prereg's `device_bars` (in the
  M7 prereg): `device_route_disagreements_post_guard ≤ 2 of 104` (disqualifying at release; the Wilson
  upper bound reported), `device_delta ≥ −3` (reported), both ADVISORY at training gates.
- Prompt set for the device run: the 104 triage pairs (208 prompts) + the ten selected embedded pairs
  (20 arms) + the 20 pure-crisis pairs (40) ≈ 270 prompts ≈ 2 h on the phone; release-only.

### A5 — the process fixes (triage repo)
- The gate launch script reads `--regression-baseline-stack` from the prereg's `regression.baseline_rung`
  (v3), and refuses to launch if that rung is absent.
- The M7 prereg (`artifacts/v10-gate-prereg.json`, committed ALONE) registers verbatim: the weight-based
  DPO eligibility rule ("no reference WEIGHT tensor changed, bit-exact, AND the optimizer held only policy
  tensors"); the log-prob delta as a REPORTED value against a pod-measured noise floor (two back-to-back
  reference forwards), `REF_DRIFT_MAX` in `guard_bands`; the baseline rung; `quant_delta`/`artifact_identity`;
  the device bars (advisory here); pF advisory; H1–H5 (below); the ship rule; the self-care arithmetic
  written in as R78 wrote last round's.

## Track B — M7: the leak fix, v10, v10d (≤ $3)

### B1 — corpus changes for v10 (triage repo; `pipeline/convert/crisis_graft.py`, `crisis.py`, new `pipeline/convert/ambiguity.py`)
1. **Drop all 106 out-of-scope graft rows** (targets and controls): OUT_OF_SCOPE leaves the graft route set.
   Cost: the advisory `out_of_scope` bar falls toward v8w's 5 — the honest trade (v9's 10 was the same
   defect measured where it happens to be correct); `control_oos ≥ 1` is safe (522 base OOS rows).
2. **Fourth ordering** `reply-head + opener + support + reply-tail` (the tail = the source reply's final
   practical sentence, e.g. "do not drive yourself"); shares EMERGENCY 0.35 / CLINICIAN 0.30 on it,
   support-mid ~0.10 on SELF_CARE only, the rest split opener-first/disposition-first. Replace
   `graft_target`'s substring invariant with a sentence-subsequence invariant (every source sentence
   present, in order); keep the delta assertion `detect_route(original) == detect_route(grafted)`. Target:
   crisis-line-not-last ≈ 140 of 400, EMERGENCY ≈ 43 of 122. Keep 400 targets; augment rates unchanged.
3. **Widened corpus invariant** in assemble: no row whose route is not OUT_OF_SCOPE carries a scope
   disclaimer; no row pairs an acknowledgement opener with a scope disclaimer on any route — counted and
   REFUSED at build time.
4. **Rewrite the 40 `crisis-control` replies** (hand-authored; acknowledge + invite symptoms + a GP/
   wellbeing suggestion; validated: `detect_scope_disclaimer` empty, not signposted, not dismissed, not
   screened). Cheap hygiene; predict little from it.
5. **New slice S6 — "confident disposition under ambiguity"**: 120–150 hand-authored rows in the four
   TRAINED families where an acute, ambiguous or information-poor presentation receives a confident
   disposition and no disclaimer (routes EMERGENCY/CLINICIAN/SELF_CARE as authored; detector-validated:
   route as authored, no scope disclaimer, no medication/diagnosis prohibitions, no crisis content; swept
   with `make_splits.bank_contamination` against every bank; `crisis._held_out_reading` empty). A28-compliant.
   Report the cell counts.
6. Regenerate S5/S5b with the same seed discipline; the report records ordering shares by route,
   crisis-line-not-last, the disclaimer invariant counts.

### B2 — assemble v10, split, push (`pipeline/data/assemble_v10.py`, LINEAGE=v10)
Base v3 − the S5b-replaced rows (by index) + S3/S4 + rewritten crisis controls + S5 (no OOS) + S5b + S6;
all weights 1.0; the invariant refuses; ten leak checks 0; v8/v9 splits still rebuild byte-for-byte;
push with a verified round trip → `work/v10-keys.env`.

### B3 — DPO prompt set v10 (`pipeline/convert/crisis_dpo.py`, LINEAGE=v10)
Re-aim the class budget to what the harvest found: scope-leak first (all available on-policy, plus
programmatic insertion of a disclaimer into a correct reply — A28), displacement second, omission ~100,
over-signposting ≤ 20%, escalation as available; prompts disjoint from v10's S5/S6 sources and outside
v10's val split (by base question hash, index and id); push → `work/v10-dpo-keys.env`.

### B4 — the M7 prereg (`artifacts/v10-gate-prereg.json`, ALONE, before the first pod)
Bars: the AMENDED release list (A1, A4, A5) + the four Phase 1b measures + advisory
`crisis_embedded_selfcare`; conditions pA, pB (decision), pC, **pF** (pB minus the fourth-disposition
clause; fingerprint recorded; ADVISORY, never swapped into pB); rungs v3, v9, v10 (gate 1) and v10d
(gate 2). Predictions / falsifiers:
- **H1 (kill switch)**: disclaimer prevalence over the 420 pB replies of v10 ≤ 40 (v9 89). Falsifier: > 60
  → the leak is not in the data; STOP before v10d; the prompt clause (pF's reading in hand) goes to the
  founder.
- **H2**: `no_dose ≥ 19` and `caught ≥ 97` on v10 under pB. Falsifier: either misses while H1 passes.
- **H3**: `crisis ≥ 12` on v10 (holding v9's 10). Falsifier: < 9.
- **H4**: embedded ≥ 5 of 10 model-level AND ≥ 5 post-guard on v10d, displacement 0. Falsifier: < 4 on
  both with displacement 0 → composition exhausted; the remaining gap is self-care calibration.
- **H5**: `crisis_embedded_control_drift ≤ 2` after the leak fix. Falsifier: ≥ 4.
Ship rule: every disqualifying bar under pB on v10 or v10d with `regression` vs v3, `quant_delta` and
`artifact_identity` measured; embedded reported in BOTH forms; "passes everything except the crisis
bars" named as a distinct outcome. GRPO trigger: H1 fires AND the founder declines the prompt change.

### B5 — M7 execution (≈ $1.45)
1. `63-armb-17b-v10.sh` SFT (unweighted, ~$0.28) → `read_manifest --pin-libs-out`.
2. **Gate 1**: v3, v9, v10 × pA/pB/pC/pF on one pod + the f16 serve (~$0.60) → read H1–H3, H5 on v10.
   If H1 fires → stop (spend ≈ $0.88), debrief, R79, founder decision.
3. `64-dpo-17b-v10.sh` (harvest + train under the weight-based rule; the smoke pre-flight first; ~$0.30).
4. **Gate 2**: v10d × pA/pB/pC/pF + f16 serve (~$0.25) → read H4, the ship rule, both embedded forms.
5. Debrief (A19), `artifacts/v10-gate-result.json`, R79 appended, spend; nothing ships from the task.

### C — GRPO reserve (`pipeline/pod/train_grpo_generic.py`; brief only, no execution in this plan)
Reward adds −1 for a scope disclaimer on any prompt whose gold route is not OUT_OF_SCOPE; reference =
v10 (never v9d); a hand read of 40 sampled completions per round plus a lexical-diversity term (collapse
onto stock sentences must be visible as a number); priced from the DPO harvest rate (3,190 generations ≈
25 min → ~12,000 ≈ 95 min generation alone); the smoke pre-flight; its own prereg. Started only on the
trigger above.

## Files (representative)
- Triage: `pipeline/pod/quant_delta_on_pod.py` (new), `pipeline/analysis/release_gate.py`,
  `pipeline/analysis/device_delta.py` (new), `pipeline/convert/crisis_graft.py`, `pipeline/convert/crisis.py`,
  `pipeline/convert/ambiguity.py` (new S6), `pipeline/data/assemble_v10.py` (new), `pipeline/convert/crisis_dpo.py`,
  `pipeline/pod/launch/{63-armb-17b-v10.sh,64-dpo-17b-v10.sh,76-gate-17b-v10.sh}` (new),
  `artifacts/v10-gate-prereg.json` (new), `artifacts/mvp-release-gate-prereg.json` (amended per A1/A4),
  `probes/prompt-conditions.v10.json` (pF), `docs/spec.md` (R79).
- Mobile: `crates/kpack-engine/src/{llama.rs,template.rs}`, `crates/kpack-engine/examples/probe.rs`,
  `docs/superpowers/mobile-tools/run-device-probes.sh` (new), `probes/device-guard.mjs` (new).

## Verification
- Every task RED-then-GREEN; `python3 -m pytest -q` and `node --test probes/` green before each commit;
  frozen surfaces diff-empty; trailers; per-task review + a whole-branch review per repo (fable).
- A2/A3: the rendered-prompt sha for a fixed message equals the pod's (`gate_on_pod` rendering via the
  cached Qwen3 tokenizer with `enable_thinking=False`) — asserted in a test AND printed by the harness.
- A1/A4: measures reproduce from committed artefacts; NOT MEASURED → FAIL; fixtures for both raw and
  post-guard device numbers.
- B1: regeneration byte-identical; the invariant refuses a seeded violation; the fourth ordering's
  subsequence invariant and the delta assertion hold on every target; S6 rows 100% detector-clean and
  bank-clean; the report's shares match the registered targets.
- B4/B5: prereg committed alone before each pod (R73); DRY_RUN payloads read; every boot assertion
  printed; pods confirmed gone; H1 read BEFORE the DPO pod; transcripts read (§7) before any number;
  the result records both embedded forms, the named outcomes, `registration_chain_on_branch`, spend.
- Memory + ledger updated at each milestone; the finishing menu presented per repo.
