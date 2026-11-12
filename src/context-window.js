// Fitting a conversation into the model's context window (§7 S7-4, D-4).
//
// # The policy (founder decision D-4)
//
// **Truncate-oldest.** When the rendered prompt would exceed `n_ctx`, the
// oldest turns are dropped from the *request*. The on-screen transcript and
// the convstore rows are NEVER truncated — they are the product's memory, and
// a P3 feature (history → RAG at link time, once device sync lands) depends on
// the full transcript still existing. Matching storage to the window would be
// an "optimisation" that destroys that feature's input years before anyone
// tries to build it.
//
// # Why this module exists rather than living in app.js
//
// Decision D-3: logic whose failure mode is silent goes where the tests run.
// This is the sharpest example in the codebase so far. The windowing was
// always here and always correct in shape; it was parameterised with the wrong
// number, and the symptom was not a wrong window — it was a decode failing
// several screens later, surfacing as "the engine broke". Nothing about the
// windowing itself looked wrong at any point, which is precisely why it needs
// tests at more than one window size.
//
// # The bug this file's signature exists to prevent
//
// `app.js` used to hold `const N_CTX = 4096; // must match inference.rs \`-c\``.
// That comment was TRUE when written — desktop's sidecar runs one context
// size. Phase 1.2 then gave mobile a per-tier window (2048 on the floor tier),
// and nothing propagated it, because nothing could: `EngineInfo` reported no
// window at all. On the reference device the frontend budgeted ~3456 tokens of
// history against an engine holding 2048 in total.
//
// So `nCtx` is a REQUIRED PARAMETER here, not a module constant. The number
// has exactly one home — the engine — and this module is handed it. A caller
// that does not know the window cannot accidentally assume one.

/// Tokens reserved for the reply. Must equal the request's `max_tokens`, and
/// the caller passes this same constant as `max_tokens` so the two cannot
/// drift — the coupling is collapsed to one source rather than described by a
/// comment. (That distinction is the whole D-4 lesson: an adjacent constant
/// with an identical-looking "must match" comment was safe for exactly this
/// reason, while `N_CTX` was not.)
export const REPLY_RESERVE = 512;

/// Headroom for tokenizer estimate error and role framing.
export const CTX_SAFETY = 128;

/// Fallback window for a backend too old to report one. Matches desktop's
/// sidecar `-c`, which is the only context size that shipped before
/// `EngineInfo.nCtx` existed.
export const FALLBACK_N_CTX = 4096;

/// Deliberately conservative: ~3.5 chars/token OVER-estimates the token count,
/// so we under-fill and stay under the window rather than risk overflow.
export function estTokens(s) {
  return Math.ceil((s ? s.length : 0) / 3.5) + 4; /* +4 ≈ role framing */
}

/// The most-recent contiguous suffix of `messages` that fits the budget left
/// after the fixed preamble (system + greeting) and the reply reserve, plus
/// how many older messages were dropped.
///
/// Always keeps the final message (the current user turn) even if it alone
/// exceeds the budget — degenerate, and the engine will truncate that one.
/// Dropping it would mean answering a question the user did not ask.
///
/// Pure: no globals, no state reads, and `nCtx` is injected. That is what
/// makes it testable at both shipping window sizes, which is the test that
/// would have caught D-4.
export function windowMessages(messages, systemContent, greetingContent, nCtx) {
  const budget = nCtx - REPLY_RESERVE - CTX_SAFETY
    - estTokens(systemContent) - estTokens(greetingContent);
  let used = 0;
  let startIdx = messages.length;
  for (let i = messages.length - 1; i >= 0; i--) {
    const t = estTokens(messages[i].content);
    if (i < messages.length - 1 && used + t > budget) break; // always keep the last
    used += t;
    startIdx = i;
  }
  return { sent: messages.slice(startIdx), droppedCount: startIdx };
}

/// The window the engine reports, or the pre-`nCtx` fallback.
///
/// Read through this rather than off `state.engine` directly, so there is one
/// place that decides what to do about a backend that does not report a
/// window — and so that place is testable.
export function engineWindow(engineInfo) {
  const n = engineInfo && engineInfo.nCtx;
  return Number.isFinite(n) && n > 0 ? n : FALLBACK_N_CTX;
}
