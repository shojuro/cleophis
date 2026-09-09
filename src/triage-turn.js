// src/triage-turn.js — the send path's supervised decisions, out of the DOM.
//
// Task 6 wires the guard into `app.js`. Most of that is DOM writing, but four
// decisions in it are worth arguing about and none of them need a document:
//
//   1. which banner a key names, and what happens to a key nobody recognises;
//   2. when the PROVISIONAL banner appears mid-stream, and that the
//      `[triage] time-to-route` line is emitted exactly once per turn;
//   3. how a finished assistant turn is persisted — attach or append — which
//      is the difference between one row and two;
//   4. what titles a supervised chat, given that its title must not come from
//      a second, un-gated model call.
//
// They live here so `node --test src/` covers them, and so the tutor's
// identity is a test rather than a claim: every function takes `supervised`
// (or a verdict) explicitly and returns the same thing it returned before this
// task when that flag is false.
import { BANNERS, ROUTE_TO_BANNER, routeOfPrefix } from './triage/guard.js';

/**
 * The banner key to render, normalised.
 *
 * A verdict always carries one of the four, but a verdict replayed from a row
 * written by an older build might not, and a blank banner on a triage reply is
 * the one rendering failure that reads as "no disposition". So an unrecognised
 * key becomes the banner that claims least — which is also what keeps the CSS
 * class and the copy from ever disagreeing, since both are built from this.
 */
export function bannerKey(key) {
  return Object.prototype.hasOwnProperty.call(BANNERS, key) ? key : 'out_of_scope';
}

/** The product copy for a banner key; an unknown key claims the least. */
export function bannerText(key) {
  return BANNERS[bannerKey(key)];
}

/**
 * What the streamed prefix should do to the provisional banner.
 *
 * The model states its disposition in the first clause, so the banner can be
 * on screen long before the reply finishes — and on a 4 GB device at ~5 tok/s
 * that gap is seconds, not milliseconds. `alreadyShown` is the whole state
 * machine: once a route has resolved for this turn, later deltas are inert,
 * so the log line is a per-turn measurement rather than a per-token one.
 *
 * UNCLEAR is the ONLY route that means "keep waiting". OUT_OF_SCOPE is the
 * model declining on the record and shows immediately.
 *
 * @returns {{route: string|null, banner: string|null, log: string|null}}
 *   All-null when nothing should change on screen.
 */
export function provisionalStep({ supervised = false, alreadyShown = false, prefixText = '', elapsedMs = 0 } = {}) {
  const nothing = { route: null, banner: null, log: null };
  if (!supervised || alreadyShown) return nothing;
  const route = routeOfPrefix(prefixText);
  if (route === 'UNCLEAR') return nothing;
  return {
    route,
    banner: ROUTE_TO_BANNER[route],
    // Read by the device session (Phase 3): the wall time from send to the
    // first disposition on screen. A console line and nothing else — no
    // column, nothing sent anywhere.
    log: `[triage] time-to-route ${Number(elapsedMs).toFixed(0)} ms`,
  };
}

/**
 * Which command persists this finished assistant turn, and with what.
 *
 * THE DISTINCTION THIS EXISTS FOR. On mobile the streaming checkpoint row is
 * finalized by `chat_cmds.rs::settle` BEFORE `ChatEvent::Done` is sent, so by
 * the time the front end has a verdict the row is already there and no longer
 * marked partial. `append_message` would therefore INSERT a second assistant
 * row for one turn — and `export_triage_log` would emit the guarded one while
 * `get_chat` replayed both. So a verdict with a row id is ATTACHED
 * (`attach_guard` writes the guard columns and sets `content` to the display
 * text, keeping `rawReply` inside the verdict); everything else appends, as
 * it always did.
 *
 * A turn WITHOUT a verdict appends unchanged even when a row id is present —
 * the tutor's path is not this task's to alter.
 *
 * @returns {{command: string, args: object}|null} null when there is nothing
 *   to persist (an empty turn, or a chat whose `create_chat` failed).
 */
export function persistAssistantTurn({
  chatId = null, content = '', citations = null, calculations = null, guard = null, messageId = null,
} = {}) {
  if (!content) return null;
  if (guard && messageId != null) return { command: 'attach_guard', args: { messageId, guard } };
  if (chatId == null) return null;
  const args = {
    chatId,
    role: 'assistant',
    content,
    citations: citations && citations.length ? citations : null,
    // Tauri maps this camelCase invoke-arg to `append_message`'s `tool_calls`.
    toolCalls: calculations && calculations.length ? calculations : null,
  };
  // Added ONLY for a guarded turn: an unguarded `append_message` must carry
  // the same argument set it carried before this task.
  if (guard) args.guard = guard;
  return { command: 'append_message', args };
}

/**
 * The first 48 characters of the user's message, trimmed and single-spaced.
 *
 * Cut by code point (`auto_title_chat` caps at 80 `chars()`, which is the same
 * unit) so the cut cannot leave half a character behind. Single-spacing first
 * matters more than it looks: a symptom description is often several lines,
 * and cutting at the newline would title the chat with its first three words.
 */
export function supervisedTitle(text) {
  const flat = String(text ?? '').replace(/\s+/g, ' ').trim();
  return [...flat].slice(0, 48).join('').trim();
}

/**
 * How this chat gets its title after the first exchange.
 *
 * `maybeAutoTitle` makes a SECOND completion, with its own system prompt at
 * temperature 0.3. For a supervised entry that is a second, un-gated call to a
 * model whose whole contract is that it only ever sees the pinned prompt at
 * temperature 0 — so the supervised chat is titled from the user's own words
 * instead, deterministically.
 *
 * @returns {{kind: 'model'}|{kind: 'fixed', title: string}|{kind: 'none'}}
 */
export function titlePlan({ supervised = false, source = null } = {}) {
  if (source == null) return { kind: 'none' };
  if (!supervised) return { kind: 'model' };
  const title = supervisedTitle(source);
  return title ? { kind: 'fixed', title } : { kind: 'none' };
}
