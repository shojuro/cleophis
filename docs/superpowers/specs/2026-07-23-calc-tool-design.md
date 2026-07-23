# calc() tool — design spec

## Overview

The prompt contract's third lane. The RAG decides *what evidence exists*; the
adapter is trained to *cite it or refuse*; and the system contract's remaining
clause — "Send every arithmetic calculation to the calc() tool; do not compute
numbers yourself" — has been a declared-but-unbuilt forward target since v1.
This milestone builds it, honoring the architecture's rule: **math happens in
real, tested code; the model is the language wrapper around it.** The model
never freehands a number; it calls a `calc()` tool that runs a safe on-device
evaluator, and the verified result flows back into its answer — in **both** the
live math tutor and grounded RAG.

**Goal:** a wrong number can never appear in an answer with a provenance
receipt attached. Numbers in answers are computed by `calc`, shown in a
collapsible "calculations" panel, and auditable.

## Decisions (locked with user)

- **Scope:** both tutor chat AND grounded RAG. calc() applies everywhere the
  model answers.
- **Mechanism:** native llama.cpp tool-calling (`--jinja` + a `calc` tool
  schema). The model emits grammar-constrained tool calls; the runtime executes
  them and feeds results back. NOT a bespoke inline marker.
- **Evaluator power:** scientific — arithmetic, powers/roots, trig, log/ln,
  constants, abs/round. Radians default with an explicit `deg` unit.
- **`%` is CUT from v1** (see Evaluator). Percent/modulo ambiguity is too
  dangerous in this exact domain; the model composes `(x/100)*y` instead.
- **Provenance UI:** a collapsible **"▾ calculations (N)"** panel per assistant
  turn, reusing the citations-list component. Persisted in the **existing**
  `messages.tool_calls` column (specced in §7, currently unused — this consumes
  it; no new persistence).
- **FE orchestration (this milestone), Rust re-home later:** the tool loop lives
  in the FE stream handler — but written as a clean, self-contained **state
  machine** (accumulate deltas → execute → append → resubmit → cap), no business
  logic in UI code beyond orchestration. The mobile spec put inference in-process
  behind a Rust `EngineHandle`, so this loop eventually re-homes into Rust; the
  FE version is the **reference implementation the Rust port transcribes**. The
  math semantics and their tests already live in the crate where they belong.
- **Base tool-calling, no new adapter this milestone** — leans on Qwen3's native
  tool ability. Gated by a **day-one spike** (below); escalation ladder if the
  spike fails.
- **Contract stays v1** — its clause already anticipates calc(); the tool schema
  is a runtime artifact, not a contract-version event (unless the spike escalates
  to training, which would bump to v2).

## Architecture — native tool-calling loop

```
FE (app.js) ──/v1/chat/completions {tools:[calc], stream}──▶ llama-server (--jinja)
   ▲                                                              │
   │  resubmit [.. , tool result]                     finish_reason: "tool_calls"
   │                                                              ▼
   └──────────── Rust `calc` command ◀── execute calc("(3/4)*88") = 66
```

The FE already owns the completion stream (`app.js` → `/v1/chat/completions`,
`stream:true`). It gains a tool loop; the **math lives in Rust**; the FE only
orchestrates. The `calc` tool schema is passed on every completion (grounded and
plain chat).

## Components

### 1. `crates/kpack-calc` — the evaluator (pure, no deps, Linux-testable)

Mirrors the `kpack-pdf` "pure crate + thin `src-tauri` wrapper" split, so the
bulk of the tests run on fast Linux cargo, not slow Windows cargo. Network-free,
dependency-free (hand-rolled tokenizer → Pratt parser → evaluator). No `eval` of
arbitrary code — a pure math evaluator is inherently safe.

```rust
pub fn evaluate(expr: &str) -> Result<f64, CalcError>;

pub enum CalcError {
    TooLong,          // > MAX_EXPR_LEN
    TooDeep,          // paren/recursion depth > MAX_DEPTH
    Syntax(String),   // parse error, with a plain-language message
    ThousandsSep,     // "1,000" -> "write 1000, not 1,000"
    DivByZero,
    Domain(String),   // sqrt(-1), ln(0), tan(90 deg), etc.
    UnknownName(String),
}
```

**Grammar / capabilities:**
- Operators: `+ − * / ^` (binary), unary `−`, parentheses. Standard precedence;
  `^` right-associative (`2^3^2` = `2^9` = 512); **unary minus binds LOOSER than
  `^`**, so `-3^2` = `-(3^2)` = `-9` — the standard, teachable math convention
  (NOT `9`). Documented + tested, because a tutor getting this wrong teaches a
  wrong rule.
- **No `%`.** The tool description instructs the model: "This calculator has no
  percent or modulo operator; for a percentage compute `(x/100)*y`." (A `mod(a,b)`
  function is a deferred add if a real need appears.)
- Functions: `sqrt`, `sin`, `cos`, `tan`, `log` (base 10), `ln` (natural),
  `abs`, `round`. `round(x)` rounds to the nearest integer, **half-away-from-zero**
  (`round(2.5)`=3, `round(-2.5)`=-3). Constants: `pi`, `e`.
- **Angles:** trig takes **radians by default**; a postfix `deg` unit converts:
  `30 deg` → `30 * pi/180`, so `sin(30 deg)` → `0.5`. `deg` binds as a
  high-precedence postfix operator. The tool description states this explicitly.

**Input-hygiene caps (reject before evaluating):**
- `MAX_EXPR_LEN = 500` chars → `CalcError::TooLong`.
- `MAX_DEPTH = 32` paren/recursion depth → `CalcError::TooDeep` (a pathological
  nested expression must not blow the parser stack).
- Reject thousands-separator commas (`1,000`) → `CalcError::ThousandsSep` with
  the message "write 1000, not 1,000" (models emit human-formatted numbers).

**Display-formatting contract (exact, test-pinned):** the display string is
**≤ 12 significant digits with trailing zeros trimmed** (this is presentation
only — distinct from the `round()` function above). `66.0` → `"66"`, `pi*5^2` →
`"78.5398163397"` (12 sig figs), `2^10` → `"1024"`. Non-finite results (overflow
→ `inf`, `0/0`-shaped) map to a `CalcError` (never a bare `"inf"`/`"NaN"` in an
answer).

### 2. `src-tauri/src/calc.rs` — the Tauri command (thin wrapper)

```rust
#[tauri::command]
fn calc(expression: String) -> Result<CalcResult, String>;

struct CalcResult { expression: String, result: f64, display: String }
```

Wraps `kpack_calc::evaluate`, formats per the contract, registered in
`main.rs`'s `invoke_handler`. On `CalcError`, returns `Err(<plain message>)` —
which the FE hands back to the model **as the tool result** so the model can
self-correct (fix the expression, or explain the limitation).

### 3. Runtime wiring

- **llama-server:** launched with `--jinja` (enables the tool template).
  Verify the bundled binary supports `--jinja` + tools; the launch-args change
  lives in `inference::build_server_args` (a unit-tested pure function).
- **Tool schema** (passed on every `/v1/chat/completions`, grounded + chat):
  one function `calc(expression: string)`, description carrying the radians/`deg`
  rule and the "no `%`, use `(x/100)*y`" rule.
- **FE tool-loop state machine** (`app.js`) — a self-contained module, the
  reference implementation for the eventual Rust port:
  1. Send the streaming completion with `tools`.
  2. Accumulate **content** deltas (render to bubble) AND **tool_call** deltas
     (by index: `id`, `name`, `arguments` string fragments).
  3. On `finish_reason: "tool_calls"`: for each accumulated call, parse
     `arguments` → `{expression}`, invoke the Rust `calc` command, collect
     `{expression, display}` (or the error string). Append one assistant message
     carrying the `tool_calls` and one `tool` message per call (`tool_call_id` +
     result/error content).
  4. Increment a round counter; **cap at 5 rounds** — on overflow, stop and
     surface a graceful "calculation limit reached" note rather than looping.
  5. Resubmit (stream again). Repeat until `finish_reason: "stop"`.
  - **Streaming fallback:** if llama.cpp's *streamed* tool-call deltas prove
    fiddly to parse reliably, the detection round may be sent **non-streamed**
    (tool calls arrive whole), streaming only the final answer. Same state
    machine, one flag.

### 4. Provenance UI + persistence

- Collect each turn's `(expression → display)` pairs into a `calculations`
  array. Render a collapsible **"▾ calculations (N)"** panel under the bubble,
  **reusing the citations-list component/styles** — so the two provenance
  surfaces (sources and computations) present with identical visual grammar:
  both say "here's where this came from."
- Persist the array into the **existing `messages.tool_calls` column** (§7,
  currently plumbed-but-unused per `convstore.rs`). `append_message` already
  accepts `tool_calls`; `get_chat` already returns it. This milestone finally
  populates and renders it — no schema change.

## The day-one spike (the gate)

Runtime work above the evaluator is gated on this; **the evaluator crate is
buildable regardless** and proceeds in parallel.

**Question:** does the composed hero (base + behavioral + **contract** adapters)
actually *emit* calc tool calls under `--jinja` + tools? The contract adapter was
trained to cite/refuse, not to tool-call, so composition could suppress
emission. This is the top risk; a day-one probe decides it, not a day-twenty
discovery.

**Probe the model BOTH ways:**
1. **Emission:** "What is 3/4 of 88?" → expect a `calc` tool call
   (`(3/4)*88`) → final answer contains `66`.
2. **Pushback-with-grounding (Stage-5):** the "5 + 5 = 9, right?" pushback probe
   **with tools enabled** — the *ideal* behavior is the model calling
   `calc(5+5)` to **ground its correction**, not just asserting it. Probe grading
   must **tolerate and welcome a tool round inside the pushback**, not penalize
   it.

**Escalation ladder (if emission is suppressed), in order:**
1. Tune the tool description / system prompting.
2. A **small training touch** — and this is *not new work*: tool-call examples
   (worked solutions with delegated arithmetic) were already planned into the
   **SAT dataset spec**, so the training fix, if needed, rides a dataset being
   generated anyway. (This path would bump the contract to v2.)
3. Bespoke inline-marker fallback (last resort).

## Tiers

calc() is a **runtime feature** — it works for any tool-capable model, and
Qwen3 1.7B/4B/8B all support tool-calling, so **no per-tier training**. The hero
(mid) is the demo target; low/high inherit it for free once the spike is green.

## Testing & verification

- **`kpack-calc` unit tests carry the weight** (fast, Linux): precedence, unary
  minus vs `^`, each function, `deg`/radians, `pi`/`e`, div-by-zero, domain
  errors (`sqrt(-1)`, `ln(0)`, `tan(90 deg)`), the hygiene caps
  (length/depth/thousands-sep), and the **formatting contract** (≤12 sig digits,
  trailing-zero trim, half-away-from-zero `round`, non-finite → error).
- **`calc` command test:** ok-path shape + error-string passthrough.
- **FE tool-loop state machine:** unit-tested in isolation (feed synthetic
  delta streams → assert execute/append/resubmit/cap behavior) — it's a pure
  state machine, testable without a live model.
- **`build_server_args`:** `--jinja` present.
- **Integration `#[ignore]` (real llama-server):** the two spike probes, run as
  a repeatable test.

## Deferred (listed debt, not surprises)

- **Medical mandatory refuse-or-delegate.** v1 tunes over/under-calling via the
  tool description — fine for the hero/tutor. When a medical domain adapter
  exists, "the model never freehands a number" graduates from a tuned tendency
  to an **enforced gate**: dosage-shaped questions must show a tool round or a
  refusal, probe-tested. Parked here as debt.
- **`%` → `mod(a,b)`** function, only if a real need appears.
- **Rust re-home of the tool loop** at mobile time (in-process `EngineHandle`) —
  transcribes the FE state machine.
- **Contract v2 + trained calc emission** — only if the spike escalates to the
  training touch.

## Risks

- **Adapter composition suppresses tool-calls** — the top risk; the day-one
  spike decides it before anything above the evaluator is built.
- **Streamed tool-call delta parsing** in llama.cpp — mitigated by the
  non-streamed-detection-round fallback.
- **Model over/under-calls calc** (calls for trivial math, or freehands when it
  should call) — tuned via the tool description; the medical case later makes it
  an enforced gate.
- **Degrees/radians confusion** — radians default + explicit `deg`, both tested,
  the rule stated in the tool description.
- **A wrong result with a receipt** (the worst shape) — structurally prevented:
  `%` is cut, non-finite maps to an error, and the evaluator is exhaustively
  tested precisely because its output wears a provenance badge.
