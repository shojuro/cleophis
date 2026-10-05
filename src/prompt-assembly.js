// src/prompt-assembly.js — what the model is shown, in one place.
//
// Pulled out of app.js so it can be tested: the difference between a
// supervised (triage) entry and the tutor is precisely the difference
// between the context the triage model was GATED under and the context the
// tutor app grew. For a supervised entry the system content is the catalog's
// systemPrompt and nothing else, and the greeting is UI only.
//
// Phase 1h M6 adds the LOOKUP mode, assembled by `assembleLookupMessages`
// below and never by `assembleMessages`: the triage path (pinned prompt,
// fingerprint throw) is left exactly as it was.
import { estTokens } from './context-window.js';

export function assembleMessages({ entry, groundedPrompt, sent, ungroundedNote, fingerprint = null }) {
  if (entry && entry.supervised) {
    const system = String(entry.systemPrompt ?? '');
    if (fingerprint) {
      // A supervised entry that pins NO fingerprint is not an entry that
      // passes the check — it is one that cannot be checked, and a prompt
      // nobody can check is not the prompt the gate was run under. Loud, in
      // the same shape as a mismatch, so the send path surfaces both the same
      // way and neither can become a silent send.
      if (!entry.promptFingerprint) {
        throw new Error('prompt fingerprint missing: this supervised entry pins none, so the prompt cannot be checked');
      }
      if (fingerprint(system) !== entry.promptFingerprint) {
        throw new Error(`prompt fingerprint mismatch: catalog says ${entry.promptFingerprint}, prompt is ${fingerprint(system)}`);
      }
    }
    return { system, messages: [{ role: 'system', content: system }, ...wire(sent)] };
  }
  const system = groundedPrompt != null ? groundedPrompt : `${entry.systemPrompt}${ungroundedNote}`;
  return {
    system,
    messages: [
      { role: 'system', content: system },
      { role: 'assistant', content: entry.greeting },
      ...wire(sent),
    ],
  };
}

// WHAT GOES ON THE WIRE, and nothing else. `sent` is a window over the app's
// OWN message objects — the same ones `state.chat.messages` holds — and those
// carry rendering and bookkeeping state that has no business in a prompt: the
// row `id`, replayed `citations` and `calculations`, and on a supervised turn
// the entire guard verdict, `rawReply` included. Spreading them put all of it
// into `chat_stream`'s payload and into the desktop sidecar's request body:
// a different wire shape than before any of this existed, and a supervised
// turn re-sending every earlier RAW reply to the model.
//
// So the history is projected, once, here — the one place both platforms and
// both entry kinds pass through. Anything a future field needs to reach the
// model must be added deliberately, which is the point.
function wire(sent) {
  return (sent ?? []).map((msg) => ({ role: msg.role, content: msg.content }));
}

// ── Phase 1h M6: the lookup mode ────────────────────────────────────────────
//
// A lookup turn is SINGLE-TURN. Its system content is the grounded prompt Rust
// assembled (`rag_lookup` → `assemble_system` over the triage contract, the ONE
// assembler; JS never assembles a grounded prompt), and its only other turn is
// the user's query. No history, no greeting, no catalog systemPrompt: a lookup
// never sees the triage conversation, and the triage conversation never sees a
// lookup (see `triageHistory` in triage-turn.js).

/** The lookup's context window, and the reply the entry pins (sampling.maxTokens). */
export const LOOKUP_N_CTX = 2048;
export const LOOKUP_REPLY_TOKENS = 320;

export class LookupBudgetError extends Error {
  constructor(tokens, replyTokens = LOOKUP_REPLY_TOKENS) {
    super(`lookup prompt over budget: ${tokens} estimated tokens > ${LOOKUP_N_CTX} (system + user + ${replyTokens})`);
    this.name = 'LookupBudgetError';
    this.tokens = tokens;
  }
}

/**
 * system + user + the reply's tokens (the entry's `sampling.maxTokens`, 320
 * when it pins none) must fit 2048, by the estimator the app
 * already windows with (`estTokens`, which over-counts on purpose). Trimming
 * trailing sources to fit is Rust's job (M5); here an over-budget prompt is a
 * hard error, never a silent truncation.
 */
export function lookupBudget({ system = '', query = '', replyTokens = LOOKUP_REPLY_TOKENS } = {}) {
  const tokens = estTokens(system) + estTokens(query) + replyTokens;
  return { tokens, ok: tokens <= LOOKUP_N_CTX };
}

/**
 * The lookup's messages. Takes ONLY the grounded prompt and the query: any
 * other argument (an entry, a history) is ignored by construction.
 *
 * @throws {Error} when there is no grounded prompt (never falls back to the
 *   triage prompt), and {LookupBudgetError} when over budget.
 */
export function assembleLookupMessages({ groundedPrompt, query, replyTokens = LOOKUP_REPLY_TOKENS } = {}) {
  if (typeof groundedPrompt !== 'string' || !groundedPrompt.trim()) {
    throw new Error('lookup needs the grounded prompt from rag_lookup; there is none');
  }
  const system = groundedPrompt;
  const user = String(query ?? '');
  const { tokens, ok } = lookupBudget({ system, query: user, replyTokens });
  if (!ok) throw new LookupBudgetError(tokens, replyTokens);
  return {
    system,
    tokens,
    messages: [{ role: 'system', content: system }, { role: 'user', content: user }],
  };
}
