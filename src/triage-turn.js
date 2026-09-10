// src/triage-turn.js — the send path's supervised decisions, out of the DOM.
//
// Task 6 wires the guard into `app.js`. Most of that is DOM writing, but the
// decisions in it are worth arguing about and none of them need a document:
//
//   1. which banner a key names, and what happens to a key nobody recognises;
//   2. when the PROVISIONAL banner appears mid-stream, and that the
//      `[triage] time-to-route` line is emitted exactly once per turn;
//   3. how a finished assistant turn is persisted — attach or append — which
//      is the difference between one row and two;
//   4. what titles a supervised chat, given that its title must not come from
//      a second, un-gated model call;
//   5. what happens when that persist FAILS, which is the difference between
//      a recorded verdict and a silently unguarded raw reply;
//   6. how a stored message renders when the chat is reopened — in particular
//      an assistant row that carries no verdict at all, and WHICH entry that
//      question is answered from, since the open chat and the entered model
//      routinely disagree;
//   7. whether the turn grounds, which for a supervised entry is never;
//   8. whether the entered model may answer in the chat that is open at all.
//
// They live here so `node --test src/` covers them, and so the tutor's
// identity is a test rather than a claim: every function takes `supervised`
// (or a verdict) explicitly and returns the same thing it returned before this
// task when that flag is false.
import { BANNERS, ROUTE_TO_BANNER, routeOfPrefix } from './triage/guard.js';

/**
 * A fifth banner, and the only one that is NOT a disposition.
 *
 * `guard.js` owns the four the detectors can produce, and this module does not
 * edit that file. This one is the product saying it has nothing to show: the
 * reply reaching the screen never went through the guard, so no route can be
 * claimed for it. Kept here, beside the rule that raises it.
 */
export const UNVERIFIED_BANNER = 'unverified';
export const UNVERIFIED_TEXT = 'This reply was not verified by the safety check and is not shown.';
const ALL_BANNERS = Object.freeze({
  ...BANNERS,
  [UNVERIFIED_BANNER]: Object.freeze({ title: 'Unverified reply', line: '' }),
});

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
  return Object.prototype.hasOwnProperty.call(ALL_BANNERS, key) ? key : 'out_of_scope';
}

/** The product copy for a banner key; an unknown key claims the least. */
export function bannerText(key) {
  return ALL_BANNERS[bannerKey(key)];
}

/**
 * How much wall clock must pass between two detector passes before a route
 * resolves.
 *
 * `routeOfPrefix` reads the WHOLE prefix, so running it on every delta is
 * O(n²) regex work on the UI thread for a reply that never states a
 * disposition, on a device whose UI thread is also drawing the stream.
 *
 * 100 MS OF WALL CLOCK, NOT A TOKEN COUNT, and the choice matters. What the
 * banner competes with is not tokens, it is the frame the user is waiting to
 * see, so a budget denominated in time is the one that stays true whatever the
 * generation rate. A token count does not: at 4 tok/s an every-8-tokens rule
 * would hold the banner back by up to two seconds — and `[triage]
 * time-to-route` is exactly the number the device session is there to read, so
 * that rule would corrupt the measurement it is meant to sit beside. At 100 ms
 * the line stays honest to within a tenth of a second, and a fast device (where
 * deltas really do arrive faster than the budget) is where the skipping
 * actually happens and the cost is actually worth bounding.
 */
export const ROUTE_CHECK_MIN_MS = 100;

/**
 * May the detectors run again yet?
 *
 * The throttle, as its own pure rule so it is tested once and stated once.
 * `lastCheckMs` is when they last ran, on the same clock as `elapsedMs` (both
 * measured from send); a non-finite `lastCheckMs` means they never have, which
 * is the FIRST delta and always runs — the opening clause is where the model
 * states its disposition, so the earliest possible pass is the valuable one.
 *
 * Fails toward running: a non-finite `elapsedMs` RUNS the detectors rather
 * than reading as "not yet", because the failure mode of skipping is a
 * supervised reply whose banner never appears at all. (Unreachable in the
 * product — `performance.now() - sentAt` is always finite — but the rule is
 * stated here and pinned by its test so the safe direction is the coded one.)
 */
export function shouldCheckRoute({ lastCheckMs = null, elapsedMs = 0 } = {}) {
  if (!Number.isFinite(lastCheckMs)) return true;
  if (!Number.isFinite(elapsedMs)) return true;
  return elapsedMs - lastCheckMs >= ROUTE_CHECK_MIN_MS;
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
 * BEFORE a route resolves there is no such early return, which is what
 * `shouldCheckRoute` is for. The caller owns the "when it last ran" state, the
 * way it already owns `alreadyShown`, and gates this call on that rule; the
 * `[triage] time-to-route` line then still measures the first resolution to
 * within `ROUTE_CHECK_MIN_MS`.
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
 * The sampling the desktop sidecar is asked for, given the entry.
 *
 * The in-process engine already reads the catalog's `sampling` for a supervised
 * entry; the desktop transport does not — it sends one hard-coded pair for
 * every model. That is not the shipped triage path, but it IS the path the
 * plan's own desktop check of the guard runs on, and a check that runs at
 * temperature 0.7 is not checking the model the gate was cut against (`0.0`,
 * 320 tokens). So the desktop dev check now runs under the same sampling.
 *
 * `entry.supervised === true` and nothing looser: the tutor's request body must
 * be the one it has always been, and `supervised` is a Rust `bool` that
 * serialises as `null` for every entry that does not set it.
 *
 * `Number.isFinite`, NOT `||`. The one value a supervised entry most needs to
 * pin is `temperature: 0.0`, which is falsy — `s.temperature || fallback` would
 * silently send 0.7 for exactly the entry this exists for.
 *
 * @returns {{maxTokens: number, temperature: number}}
 */
export function samplingFor({ entry = null, maxTokens = 0, temperature = 0 } = {}) {
  if (!entry || entry.supervised !== true) return { maxTokens, temperature };
  const s = entry.sampling || {};
  return {
    maxTokens: Number.isFinite(s.maxTokens) ? s.maxTokens : maxTokens,
    temperature: Number.isFinite(s.temperature) ? s.temperature : temperature,
  };
}

/**
 * The verdict as it is PERSISTED: `applyGuard`'s twelve keys plus the three
 * that say which bytes produced the reply.
 *
 * WHY IT IS NOT IN `applyGuard`. That function's shape is pinned by Task 3's
 * tests and it is pure in the strong sense — a verdict is a function of the
 * user's words and the model's, and nothing else. Provenance is a fact about
 * the TURN, not about the text, and it is only ever needed at the moment the
 * row is written. So it is stamped here, on a copy, and the twelve-key shape
 * survives untouched in memory and in every test that pins it.
 *
 * WHY BY SHA AND NOT BY ID. `export_triage_log` already writes the chat's
 * `model_id` and `adapter_ids`, and spec §5 P2.5 asks for these three as well.
 * An id is a name the catalog may re-point: Phase 3 swaps the triage adapter
 * (v3 → v8) under the same entry id, and a clinical review reading an exported
 * line months later cannot tell which adapter answered unless the bytes are on
 * the line. These come from the entry the turn was SENT under — threaded
 * through `finishStream` for the same reason `turnChatId` is — so a catalog
 * that changes mid-stream cannot rewrite the provenance of a reply already
 * given.
 *
 * Always all three, always strings, `''` when the entry pins none, so every
 * guarded row has one shape. The tutor never reaches here: a null verdict
 * returns null and `persistAssistantTurn` is called with exactly the argument
 * set it was called with before this existed.
 *
 * @returns {object|null} a NEW object; `verdict` is never mutated.
 */
export function guardForPersistence(verdict, entry = null) {
  if (!verdict) return null;
  const str = (v) => (typeof v === 'string' ? v : '');
  return {
    ...verdict,
    promptFingerprint: str(entry && entry.promptFingerprint),
    modelSha: str(entry && entry.sha256),
    adapterSha: str(entry && entry.adapterSha256),
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

/**
 * Does this turn go through the pack/RAG path?
 *
 * Never, for a supervised entry, however many packs are attached. Task 5's rule
 * is that the supervised system content is the catalog's `systemPrompt` and
 * nothing else — and grounding replaces exactly that. Two unguarded producers
 * come off with it: the grounded prompt (a different gate wearing the same
 * name) and the scripted `noEvidence` refusal, which is written straight into
 * the transcript and persisted without ever passing through `applyGuard`.
 */
export function shouldGroundTurn({ supervised = false, packCount = 0 } = {}) {
  return !supervised && packCount > 0;
}

/**
 * What to do when persisting the finished turn failed.
 *
 * ONLY `attach_guard` is surfaced, and the asymmetry is the point. When
 * `append_message` fails nothing is written, which is the app's existing
 * degrade-gracefully contract and leaves nothing behind. When `attach_guard`
 * fails the row is already there — `settle` finalized it with the model's RAW
 * reply — so a swallowed failure persists the unguarded text, shows it on the
 * next open, and drops the turn out of the export. That is the guard failing
 * toward showing MORE than it decided to show, silently.
 *
 * Retried once, because the realistic cause is a locked database rather than a
 * rejected verdict. Then said out loud. The display text stays on screen either
 * way: it was computed here and is still what this reply routes to.
 *
 * @returns {{action: 'ignore'|'retry'|'surface', log: string|null, message: string|null}}
 */
export const ATTACH_FAILED_NOTICE = 'This reply could not be verified and was not recorded — ask again';

export function persistFailurePlan({ command = '', attempt = 1, error = '' } = {}) {
  if (command !== 'attach_guard') return { action: 'ignore', log: null, message: null };
  const log = `[triage] attach_guard failed ${String(error?.message ?? error ?? '')}`;
  return attempt < 2
    ? { action: 'retry', log, message: null }
    : { action: 'surface', log, message: ATTACH_FAILED_NOTICE };
}

/**
 * The catalog entry a stored chat BELONGS to.
 *
 * `openChat` opens any chat from the sidebar without re-pointing
 * `state.chat.model`, so the entered model and the open chat's model routinely
 * disagree — in both directions. Rendering a transcript is a question about the
 * chat, so it is answered from the chat's own `modelId` here, and the entered
 * model is only the fallback for a chat whose model this catalog no longer
 * lists.
 *
 * (The first attempt at this asked "is a supervised model entered, or does this
 * transcript hold a verdict?". The second term made a tutor chat carrying one
 * stray triage row look supervised, and the first withheld every reply in an
 * ordinary tutor chat opened while triage happened to be entered. The chat's
 * own entry is the question that has an answer.)
 */
export function entryForChat(catalog, chat, enteredModel = null) {
  const id = chat && chat.modelId;
  const found = id ? (catalog ?? []).find((e) => e && e.id === id) : null;
  return found ?? enteredModel ?? null;
}

export const FOREIGN_CHAT_NOTICE = 'This chat belongs to a different assistant — start a new chat for the triage assistant.';

/**
 * May the entered model answer in the chat that is open?
 *
 * The guard follows the ENTERED model, because that is the model that answers —
 * and that is exactly why a supervised model must not answer in someone else's
 * chat. Sending would write a guarded triage row, banner and verdict and audit
 * line, into a tutor conversation; the export would then carry a triage
 * disposition from a chat nobody triaged in.
 *
 * A chat that names NO model is refused rather than assumed. Refusing costs the
 * health worker a new chat. Assuming files a triage reply somewhere nobody can
 * account for. An absent chat record is not that case: there is no chat yet,
 * and the first message creates one with this model's id.
 *
 * An unsupervised entered model is allowed everywhere, as it always was.
 *
 * @returns {{allowed: boolean, message: string|null}}
 */
export function canSendInChat({ entered = null, chat = null } = {}) {
  if (!entered || entered.supervised !== true) return { allowed: true, message: null };
  if (chat == null || chat.modelId == null) return { allowed: true, message: null };
  return chat.modelId === entered.id
    ? { allowed: true, message: null }
    : { allowed: false, message: FOREIGN_CHAT_NOTICE };
}

/**
 * How one stored message renders when a chat is reopened.
 *
 * THE ROW THIS EXISTS FOR: an assistant row in a supervised chat with no
 * verdict on it. A partial row left by a killed turn, a row whose
 * `attach_guard` never landed, a row from any producer that did not go through
 * `applyGuard` — each reaches `rebuildChatDom` as an ordinary bubble carrying
 * the model's own words with no banner at all, which is the one thing a
 * supervised reply may never be. Its content is withheld and replaced by the
 * fixed note; `rawReply` is not lost, because the row itself still holds the
 * text and the export still reads it.
 *
 * @returns {{banner: string|null, text: string, withheld: boolean}}
 */
export function replayMessage({ supervised = false, role = 'assistant', content = '', guard = null } = {}) {
  if (guard) return { banner: guard.banner, text: content, withheld: false };
  if (supervised && role === 'assistant') {
    return { banner: UNVERIFIED_BANNER, text: UNVERIFIED_TEXT, withheld: true };
  }
  return { banner: null, text: content, withheld: false };
}
