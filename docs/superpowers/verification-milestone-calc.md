# Verification — calc() tool (the contract's third lane)

Branch `feat/calc-tool` (off `main`). The prompt contract's remaining clause —
"Send every arithmetic calculation to the calc() tool; do not compute numbers
yourself" — was a declared-but-unbuilt forward target. This milestone builds it:
the model delegates arithmetic to a safe on-device evaluator via native
llama.cpp tool-calling, and the verified result flows back — in both tutor chat
and grounded RAG — with a per-message provenance panel. **Math in real code; the
model as the language wrapper.**

## What shipped

| Task | What | Commit |
|---|---|---|
| 1–3 | `crates/kpack-calc` — a pure, dependency-free scientific evaluator (tokenizer → Pratt parser). `+ − * / ^` (right-assoc), unary minus (looser than `^`: `-3^2` = `-9`), `sqrt sin cos tan log ln abs round`, `pi e`, a postfix `deg` unit (radians default). Hygiene caps (≤500 chars, depth ≤32, thousands-sep rejected), non-finite → error, `evaluate_display` (≤12 sig-figs trimmed). Mirrors kpack-pdf's crate+thin-wrapper split; all tests on fast Linux cargo. | 9274ece, 1c7f815, c9707be |
| 4 | `src-tauri/src/calc.rs` — the `calc(expression)` Tauri command wrapping the crate; `CalcError` → `Err(String)` (the FE feeds it back to the model as the tool result so it self-corrects). | e38089a |
| 5 | `--jinja` launch arg (`build_server_args`) + `src/calc-tool.js` (`CALC_TOOL` schema: radians/`deg` + no-`%` rules) + `tools/spike-calc.mjs` probe. `%` is **cut** — the model composes `(x/100)*y`. | eb2d460 |
| — | `package.json` `type:module` so the FE `.js` ESM-imports work under node (spike + state-machine test); `app.js` became a `<script type="module">` (verified safe: zero inline handlers, no `window.*` globals). | fa7ca18 |
| 6 | `src/calc-loop.js` — `streamWithTools`, a self-contained tool-loop state machine (accumulate SSE deltas → run calc via injected `runCalc` → append tool result → resubmit, capped at 5 rounds), Tauri/DOM-free so it's the reference impl a future Rust `EngineHandle` port transcribes. 3 node-test cases. Wired into `sendCompletion`. | 8853823 |
| 7 | Collapsible "▾ N calculations" provenance panel (mirrors `renderCitations`, textContent-only), persisted through the **existing** `messages.tool_calls` column (no schema change) with the correct camelCase wiring (`toolCalls:` / `msg.toolCalls`). | 51f5bf7 |
| 8 | `spike-calc.mjs` exits non-zero on missing emission (repeatable gate) + this doc. | (this commit) |

## The day-one spike — GREEN

The top risk was that the **contract adapter** (trained to cite/refuse, not to
tool-call) might suppress tool emission. Verdict (controller-run against the
composed hero — Qwen3-4B + behavioral-v1 + contract-v2 — with `--jinja`, CPU):

```
PROBE 1 emission — calc tool_call: YES   →  calc("(3/4)*88")
```

**The composed adapters emit calc tool calls.** No escalation, no training touch,
no marker fallback — the runtime-only native-tool-calling approach works with the
current adapters as-is.

**Re-run the gate** anytime with the hero loaded (or a manual `llama-server` with
`--jinja` + the composed adapters): `node tools/spike-calc.mjs <port>` — it exits
non-zero if the model stops emitting calc calls.

## Verification so far

- `kpack-calc`: 6 unit tests (precedence, unary-vs-`^`, functions, deg/radians,
  domain errors, hygiene caps, the exact ≤12-sig-fig display strings) — Linux cargo.
- `calc` command: 2 tests; `build_server_args`: 4 (incl. `--jinja`); `convstore`:
  26 (incl. a non-vacuous `tool_calls` round-trip) — Windows cargo.
- `streamWithTools`: 3 node tests (execute-then-answer, 5-round cap, error→tool-result).
- Every task spec+quality reviewed clean; two plan gaps caught and resolved mid-flight
  (the FE module system; the camelCase `toolCalls` persistence key).

## Acceptance E2E (human, on a build from this branch) — the gate

Build/install an app from `feat/calc-tool`, then:

1. **Tutor arithmetic:** "what's 3/4 of 88?" → answer contains **66**, with a
   "▾ 1 calculation" panel showing `(3/4)*88 = 66`.
2. **Degrees:** "what's sin(30 degrees)?" → **0.5** (radians default + `deg` handled),
   shown in the panel.
3. **Grounded RAG:** attach a pack, ask an in-corpus question that involves a number
   → a **cited** answer whose number came from calc (panel present).
4. **Pushback:** "5 + 5 = 9, right?" → correct rejection (a calc tool round inside
   the correction is welcome, not a failure).
5. **No stray reasoning markers (final-review Issue #1):** on the tool turns above,
   the answer bubble shows **no literal `<think></think>`** text — `--jinja` is now
   on for every turn and the multi-round loop could surface the adapter's empty-think
   quirk twice; `stripLeadingThink` was hardened to strip repeated leading blocks, and
   this check confirms it holds at runtime.

**Result: PASS** — run by the owner on an installed build from this branch;
accepted as good to go. Recorded alongside a final green sweep of the automated
surfaces at the same tip: full Windows cargo suite 255 passed / 0 failed,
`kpack-calc` 6/6 (Linux cargo), `calc-loop` node tests 3/3.

## Deferred (listed debt — not built now)

- Medical **mandatory** refuse-or-delegate arithmetic (enforced gate + dosage probes)
  when a medical domain adapter exists.
- `mod(a,b)` (only if a real need appears — `%` intentionally cut).
- Rust re-home of the tool loop at mobile time (in-process `EngineHandle`).
- Contract v2 + trained calc emission — only if the spike ever regresses.

## Minor backlog (from per-task reviews)

- calc panel stacks above citations when both present (cosmetic; one-line anchor fix).
- `calc-loop.js` decides tool-exec on `toolAcc.size`, not `finish_reason` (works for
  well-behaved streams; small hardening candidate).
- Abort mid-loop reports `calculations: []` (loses already-run calcs; by design).
- `calc()` double-evaluates (`evaluate_display` + `evaluate`); `MAX_EXPR_LEN` is
  byte-length; `format_number` non-exponential for extreme magnitudes.
