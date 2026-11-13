# Vision Input — Stage 3: Probe Set, Scorer, Device Run, CI, Calibration — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A licence-clean, pinned probe set with a pure scorer that turns the transcriber's output into a pass/fail against the spec's gates; one command that runs it on the phone and one CI job that runs it on Linux; and the confidence thresholds calibrated against measured readings instead of the plan's provisional numbers.

**Architecture:** Fixtures are typeset from our own LaTeX with matplotlib's mathtext (no TeX on the box) by a committed generator, so they are reproducible and licence-clean; `items.json` carries the expected verdict and LaTeX; `manifest.json` pins every file by sha256 and the test enforces it. The scorer is a pure Node module (the normalisation rules ported from Stage 0's `score-spike.py`, an explicit equivalence list, sign/operator substitutions weighted ×5, verdict correctness, "survives to the tutor" = an accepted reading with a sign error). The bench's `--via-engine` path (the product's own `prepare → transcribe → parse → assess` chain) gains a JSONL writer so the device run and CI score the product verdict, not a re-implementation. The device script and the CI job are clones of `run-vision-bench.sh` and `determinism.yml`. Calibration is one measured decision at the end, recorded with its evidence.

**Tech Stack:** matplotlib 3.10 mathtext (Python 3, Agg backend), Node 20 (`node --test`), the `kpack-engine` `vision_bench` example (`--features vision`), GitHub Actions (`ubuntu-latest`, `ubuntu-24.04-arm`), the public dist bucket for the two GGUFs.

**Spec:** the approved staged plan's "Stage 3" section (`/home/penguinzyue/.claude/plans/keen-baking-karp.md`, copied into `docs/superpowers/specs/` by Stage 1) and the Vision Input Spec v0.2 gates: ≥ 18/20 exact after normalisation; sign/operator errors weighted ×5; zero sign errors accepted; refusals correct on the diagram-only and non-maths images; floor-tier latency ≤ 8 s p50 / 12 s p95; peak ≤ 1.2 GB VmHWM for model + projector + context. Stage 0's measured regime: 512 px longest edge, 4 threads, one encoder pass, single-formula stop (`docs/superpowers/verification-vision-spike.md`).

## Global Constraints

- Worktree `~/cleophis-wt/vision-s3`, branch `vision/stage3` (from `origin/main` 4aadcc9). Never `cd` into the original repository roots. Never bare `git stash`. No secrets in the repo; CI has no secrets, so every model it needs is public.
- The repo is public: every fixture is typeset from LaTeX we wrote, or a drawing we made; no photographs of third-party material. The two founder photographs of the printed sheet are added later under `probes/vision/fixtures/v1/photos/` when supplied (self-licensed CC0), with ids `q17-photo-upright` and `q18-photo-rotated`; the manifest and `items.json` are extended then. Until then the set is 18 items and the exact-match gate is scaled: ≥ 17/18.
- Pure modules only in `probes/vision/*.mjs` (no `invoke`, no DOM); tests first (RED → GREEN); `npm test` discovers `probes/**/*.test.mjs`.
- Engine changes: `cargo test -p kpack-engine`, the Android `-D warnings` check (from `src-tauri/`, after `source ../.superpowers/ndk-env.sh`), and the host `vision_bench --via-engine` run over the fixtures (the engine crate builds on WSL with `--features vision`). The Windows desktop suite is required only if `src-tauri` changes (calibration touches `src/vision-confirm.js` and `crates/kpack-engine`, not `src-tauri`).
- Thresholds live in exactly two places that a test ties together: `vision_parse.rs` (`ACCEPT_MIN_CONFIDENCE`, `OPERATOR_MIN_PROB`) and `src/vision-confirm.js` (`CONFIDENCE_FLAG`, the operator threshold constant). Changing one without the other fails `npm test`.
- Linux CI is a transcription-regression gate, never a latency or memory gate; the workflow header says so.
- Commit trailers (last lines, this order): `Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>` then `Claude-Session: https://claude.ai/code/session_01W9ZnszzonHJc3yCaT3d2qM`. Commit after every task.

## File structure

| File | Responsibility |
|---|---|
| `probes/vision/fixtures/v1/gen.py` | typesets the 18 items deterministically (fixed font, dpi, size, seed) and writes `items.json` + `manifest.json` |
| `probes/vision/fixtures/v1/*.png`, `items.json`, `manifest.json` | the set and its pins |
| `probes/vision/manifest.test.mjs` | sha256 + count of every fixture file match the manifest; `items.json` is well-formed |
| `probes/vision/normalise.mjs` (+ test) | the pinned normalisation and the equivalence list |
| `probes/vision/score.mjs` (+ test) | per-item scoring, the gates as exported constants, the Markdown + JSON report, the CLI |
| `crates/kpack-engine/examples/vision_bench.rs` | `--via-engine --json <out.jsonl>` writes one record per item with the product's verdict, warnings, blocks (confidence, min operator prob), raw, timings, VmHWM |
| `docs/superpowers/mobile-tools/run-vision-probes.sh` | sha-verified push of bench + models + fixtures, the device run, the pull, the device-mode score |
| `.github/workflows/vision-regression.yml` | the Linux job on both architectures |
| `crates/kpack-engine/src/vision_parse.rs`, `src/vision-confirm.js` | the calibrated thresholds (Task 6, only if the evidence says so) |
| `docs/superpowers/verification-vision-stage3.md` | the record |

---

### Task 1: The fixture set

**Files:**
- Create: `probes/vision/fixtures/v1/gen.py`, the 18 PNGs it writes, `probes/vision/fixtures/v1/items.json`, `probes/vision/fixtures/v1/manifest.json`, `probes/vision/manifest.test.mjs`

**Interfaces:**
- Produces: `items.json` = `{ "version": 1, "items": [ { "id", "file", "kind": "printed"|"diagram"|"prose", "latex": "<expected LaTeX or null>", "plain": "<a reading in words>", "verdict": "accept"|"refuse", "notes" } ] }`; `manifest.json` = `{ "version": 1, "count": 18, "files": { "<file>": "<sha256>" } }`.

The 16 printed items (ids `q01`…`q16`), typeset with mathtext at 512×160 px, white background, DejaVu Sans at 28 pt, centred, each a formula crop (the measured regime):
`q01-linear` `3x - 7 = 2x + 5`; `q02-fraction` `\frac{2x}{3} + \frac{x}{6} = 5`; `q03-quadratic` `x^2 - 5x + 6 = 0`; `q04-exponent-times` `2^{n+1} \times a_n \le 10`; `q05-system` two lines `x + y = 7` / `x - y = 1` (typeset as two stacked lines; expected LaTeX joins them with `\\`); `q06-sqrt` `\sqrt{x + 1} = 3`; `q07-negative-coeff` `-4x + 9 = -7`; `q08-mixed-fraction` `\frac{3}{4}x - \frac{1}{2} = \frac{5}{8}`; `q09-subscripts` `a_1 + a_2 = 2a_3`; `q10-decimal` `0.5x + 1.25 = 3`; `q11-parentheses` `2(x - 3) = 4(x + 1)`; `q12-inequality` `3x - 2 \ge 7`; `q13-power-of-fraction` `\left(\frac{1}{2}\right)^x = 8`; `q14-percent` `15\% \times 80 = x`; `q15-text-and-formula` the line `Solve for x: 5x = 35` (expected latex `5x = 35`, the prose outside the maths); `q16-cubic` `x^3 - x = 0`.
The two refusals: `q17-diagram-only` a labelled right triangle drawn with matplotlib lines (no equation) → verdict `refuse`, latex null; `q18-prose` three lines of plain English (a shopping list) → verdict `refuse`.

- [ ] **Step 1: RED** — `manifest.test.mjs` reads the manifest and every file; asserts count 18, every sha256 matches, every `items.json` entry names an existing file with the fields above and a verdict in the set; the test FAILS before the fixtures exist.
- [ ] **Step 2: `gen.py`** — deterministic (`matplotlib.use('Agg')`, fixed `figsize=(5.12,1.6)`, `dpi=100`, `fontsize=28`, `savefig(..., metadata={'Software': None})` so the PNG carries no timestamp); re-running it reproduces identical bytes (assert in a `--check` mode that re-renders to a temp dir and compares sha256s). Writes `items.json` and `manifest.json`.
- [ ] **Step 3: GREEN** — `python3 probes/vision/fixtures/v1/gen.py && python3 probes/vision/fixtures/v1/gen.py --check && npm test`. **Step 4: Commit** — `vision(s3-1): the probe set — 16 typeset problems, a diagram, a page of prose; pinned by sha256`.

---

### Task 2: The scorer

**Files:**
- Create: `probes/vision/normalise.mjs`, `probes/vision/normalise.test.mjs`, `probes/vision/score.mjs`, `probes/vision/score.test.mjs`

**Interfaces:**
- `normalise.mjs`: `export function normalise(latex)` — ports Stage 0's rules in this order: strip `<loc_\d+>`; drop `\begin{array}{…}` and any `\end{…}` (tolerant of a truncated `\end{`); drop `\left`, `\right`, `\displaystyle`; drop thin spaces `\, \; \!`; `\dfrac`/`\tfrac` → `\frac`; drop `\text{…}`; `\colon` → `:`; strip a leading label `^[A-Za-z ]*:\s*`; join lines; apply the equivalence list `EQUIVALENCES = [[/\\cdot/g, '\\times'], [/\\leq/g, '\\le'], [/\\geq/g, '\\ge'], [/\\frac(\d)(\d)/g, '\\frac{$1}{$2}'], [/\\neq/g, '\\ne']]`; remove all whitespace. `export const EQUIVALENCES`.
- `score.mjs`: `export const GATES = { exactMin: 17, exactOf: 18, acceptedSignErrorsMax: 0, refusalsMin: 2, floorP50Ms: 8000, floorP95Ms: 12000, floorVmHwmKb: 1_258_291 }`; `export function scoreItem(item, record)` → `{ id, exact, cer, signSubstitutions, operatorSubstitutions, weightedErrors, verdictExpected, verdictGot, verdictOk, survivesToTutor }` where `cer` is Levenshtein distance over the normalised strings divided by the expected length; sign substitutions are `+`↔`-` and `\times`↔`+`/`-` mismatches counted on the aligned diff, weighted ×5; `survivesToTutor = record.verdict === 'accept' && signSubstitutions > 0`; `export function scoreRun(items, records, { device = false } = {})` → `{ rows, exact, total, refusalsOk, survivesToTutor, pass, latency: {p50, p95} | null, vmHwmKbMax | null, failures: [strings] }` with `pass` false on any gate miss; `export function toMarkdown(result)`; `export function toJson(result)`. CLI (`node probes/vision/score.mjs --items <items.json> --records <jsonl> [--device] --out-md <path> --out-json <path>`), exit 1 on a failed gate, 2 on a missing record (every item must have exactly one record per repeat; the CLI takes the LAST repeat per id).
- Records are the bench's JSONL (Task 3): `{ id, verdict, warnings, blocks: [{kind, content, confidence, min_operator_prob}], raw, total_ms, prefill_ms, decode_ms, vmhwm_kb, … }`; the scored LaTeX is the join of `latex` blocks' content with `\\`.

- [ ] **Step 1: RED tests** — normalise: each rule by example (`\dfrac{1}{2}` → `\frac{1}{2}`; `<loc_12>x` → `x`; `Solve: 5x=35` → `5x=35`; `a\le b` and `a\leq b` equal; `\frac12` → `\frac{1}{2}`); score: a sign flip (`3x+7` vs `3x-7`) accepted → `survivesToTutor` true and `pass` false; the same in review → `pass` true (if the rest passes); a missed refusal (diagram accepted) → `refusalsOk` false; exact count below the gate → `pass` false; device mode with p95 above 12,000 → `pass` false; CER arithmetic on one example. `npm test` → FAIL.
- [ ] **Step 2: Implement.** **Step 3: GREEN.** **Step 4: Commit** — `vision(s3-2): the scorer — pinned normalisation, ×5 sign errors, the gates as constants`.

---

### Task 3: The bench writes the product verdict

**Files:**
- Modify: `crates/kpack-engine/examples/vision_bench.rs` (the `run_via_engine` path)

**Interfaces:**
- `--via-engine --json <path>` appends one JSON line per item and repeat: `{ "id", "file", "rep", "verdict": "accept"|"review"|"refuse", "warnings": [snake_case names as the engine's `Debug` lowercased, e.g. "low_confidence"], "blocks": [{ "kind": "latex"|"text", "content", "confidence", "min_operator_prob": number|null }], "raw", "n_image_tokens", "stop", "prep_ms", "prefill_ms", "decode_ms", "total_ms", "vmrss_kb", "vmhwm_kb", "error": null|string }`. The assessment comes from the engine's own `vision_parse` (the same function `vision_cmds` uses) — no duplicated thresholds. Items come from `--items <items.json>` (Task 1's shape: `items[].file` relative to the json's directory).

- [ ] **Step 1: RED** — a unit test in the example (or in `vision_parse.rs`) that serialises one assessment through the record shape and checks the key names and the `min_operator_prob: null` case. **Step 2: Implement.** **Step 3: Host run** — `cargo run -p kpack-engine --features vision --release --example vision_bench -- --via-engine --family docling --model ~/cleophis-artifacts/vision/granite-docling-258M-Q8_0.gguf --mmproj ~/cleophis-artifacts/vision/mmproj-granite-docling-258M-Q8_0.preproc512.gguf --items probes/vision/fixtures/v1/items.json --max-side 512 --threads 4 --one-pass --json /tmp/…/host-v1.jsonl` (the two files exist from Stage 0/1; if the projector file is named differently there, `ls ~/cleophis-artifacts/vision/`); then `node probes/vision/score.mjs --items … --records host-v1.jsonl --out-md … --out-json …`. Record the host result in the report (it is Task 6's first evidence). **Step 4: Gates** — `cargo test -p kpack-engine`, the Android check. **Step 5: Commit** — `vision(s3-3): vision_bench --via-engine writes the product verdict as JSONL`.

---

### Task 4: The device run script

**Files:**
- Create: `docs/superpowers/mobile-tools/run-vision-probes.sh`

- [ ] **Step 1:** clone `run-vision-bench.sh`'s structure (sha-verified pushes with on-device sha256, re-run skips, the `ADB`/`SERIAL` env, the INCOMPATIBLE-CPU refusal) with: `--via-engine --json` always; the fixture dir pushed whole; the pull of the JSONL to `--out`; then `node probes/vision/score.mjs --device --items … --records <out> --out-md <out>.md --out-json <out>.json`; one `adb shell dumpsys meminfo <pkg-less: the bench pid>` reading captured into the report as the WebView-less number. Usage header names the exact founder command for the A22 (`SERIAL=R9WR50C0W1J`, `ADB=adbw` — the Windows-side adb wrapper that exists on this laptop; paths pushed must be WSL paths read by the Linux `adb` OR staged under `/mnt/c` for `adb.exe` — the header says which, as `run-vision-bench.sh`'s does). **Step 2:** `bash -n` and `shellcheck` if present. **Step 3: Commit** — `vision(s3-4): run-vision-probes.sh — the probe set on the phone, scored with the device gates`.

---

### Task 5: CI

**Files:**
- Create: `.github/workflows/vision-regression.yml`

- [ ] **Step 1:** from `determinism.yml`: `on: pull_request` + `push` path-filtered to `crates/kpack-engine/**`, `third_party/llama-cpp-sys-2/**`, `probes/vision/**`, the workflow itself; plus `schedule` weekly; matrix `ubuntu-latest`, `ubuntu-24.04-arm`; cache the two GGUFs under `~/cleophis-vision-models/` keyed by sha; on miss `curl -fL --retry 3` from `https://cleophis-dist.s3.us-east-005.backblazeb2.com/vision/granite-docling-258M/v1/granite-docling-258M-Q8_0.gguf` and `…/mmproj-granite-docling-258M-Q8_0.preproc512.gguf`; verify `b687245c212b59da938e411e2d4485596cbaeed1ae52706582ac38ed6a970f76` and `bb75a65aa3ec14ac4a002248d5ffa075183df369030b6ca09028463ede18b307` with `sha256sum -c` on hit AND miss; Rust 1.97.1 + cmake + libclang as `rust.yml` installs them; `cargo build -p kpack-engine --features vision --release --example vision_bench --locked`; run over the fixtures (`--threads 4 --max-side 512 --one-pass`); `node probes/vision/score.mjs` (no `--device`); a guard that the JSONL has exactly 18 records; upload the Markdown report as an artifact. Header comment: "transcription regression only — never latency or memory; those are the device run's". **Step 2:** `actionlint` if present, else `python3 -c 'import yaml;yaml.safe_load(open(...))'`. **Step 3: Commit** — `ci(vision): vision-regression — the probe set on both Linux architectures, scored`.

---

### Task 6: Calibration and the record

**Files:**
- Modify (only if the evidence says so): `crates/kpack-engine/src/vision_parse.rs`, `src/vision-confirm.js`, their tests
- Create: `docs/superpowers/verification-vision-stage3.md`

- [ ] **Step 1: Evidence** — the host run from Task 3 (and the device run's JSONL if the founder has run Task 4 by then; if not, say so). For every item: expected vs got (normalised), verdict, per-block confidence and min operator prob. Tabulate: the lowest confidence among EXACT readings, the highest among WRONG readings; the same for `min_operator_prob` against sign/operator errors. **Step 2: Decide** — keep 0.90 / 0.70 / 0.97 unless a threshold would (a) let a sign error through as `accept` (then raise) or (b) push ≥ 2 exact readings into `review` with no wrong reading between (then consider lowering, but never below the highest wrong reading + 0.02). Record the decision with the numbers. If a change is made, it is ONE commit touching both homes and the cross-language test, with `cargo test -p kpack-engine`, `npm test`, and the Android check green. **Step 3: The record** — what each task built, the gates verbatim, the host scoreboard (exact count, CER, substitutions, refusals), the CI job's first run link, the device command for the founder, the calibration decision, what Stage 4 (the spec's v0.2: handwriting, two-photo) inherits. **Step 4: Commit** — `docs(vision): Stage 3 record — the probe set's first scoreboard and the threshold decision`.
