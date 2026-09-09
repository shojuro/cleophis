// src/triage-confirm.js — the health worker's decision, out of the DOM.
//
// Task 6 put the guard on the screen. This module is what a person does about
// it: the model routes, the product decides what reaches the screen, and the
// HEALTH WORKER decides what the route is. Nothing here infers a confirmation —
// it is an act, written to the message row through Task 7's `confirm_route`,
// and until it is written the reply carries the model's route and says so.
//
// Everything worth arguing about lives here rather than in `app.js`, for the
// same reason Task 6's decisions live in `triage-turn.js`: `app.js` reaches for
// `window.__TAURI__` at module scope and cannot be imported under
// `node --test`, so `app.js` is left with the DOM writes and this file carries
// the rules.
//
//   1. WHICH ROUTES the picker may offer, which is exactly the set
//      `convstore::ConvStore::confirm_route` accepts — UNCLEAR is not one;
//   2. WHEN the controls exist at all (a row id to write against) and when they
//      are locked (a confirmation already recorded);
//   3. WHAT THE RECEIPT SAYS — the sentences the guard removed, quoted as
//      written, plus what it added, because a reader cannot see either;
//   4. what to do with `confirm_route`'s return value, which today is nothing;
//   5. whether the triage-log export is offered at all, and with which
//      arguments — the front end picks a destination and reshapes no bytes.
//
// Every entry point takes `supervised` explicitly and returns the same
// "nothing" for an unsupervised chat, so the tutor's identity is a test rather
// than a claim.

/**
 * The four routes the contract defines, in `confirm_route`'s own order, with
 * the words a health worker reads.
 *
 * A fifth entry here would be a route the front end can offer and the store
 * must refuse — see `convstore.rs`'s `ROUTES`.
 */
export const ROUTE_LABELS = Object.freeze({
  EMERGENCY: 'Emergency',
  CLINICIAN: 'See a clinician',
  SELF_CARE: 'Self-care',
  OUT_OF_SCOPE: 'Cannot judge',
});

export const CONFIRM_ROUTES = Object.freeze(Object.keys(ROUTE_LABELS));

function isContractRoute(route) {
  return typeof route === 'string' && Object.prototype.hasOwnProperty.call(ROUTE_LABELS, route);
}

/**
 * The words for a route, whatever it is.
 *
 * NOT normalised, and the difference from `modelRouteOf` below is the point.
 * Which route to OFFER is a question about this build's contract, so an
 * unrecognised one becomes the route that claims least. What a RECORDED
 * decision SAYS is a question about a person's own act: printing "Cannot judge"
 * over a stored route this build does not know would misreport the one thing on
 * screen that a human put there. So the token is shown as itself.
 */
export function routeLabel(route) {
  return isContractRoute(route) ? ROUTE_LABELS[route] : String(route ?? '');
}

/**
 * The route to pre-select — the model's, expressed in the four the store takes.
 *
 * UNCLEAR is a detector answer rather than a disposition and `confirm_route`
 * refuses it; `ROUTE_TO_BANNER` already shows CANNOT JUDGE for it, so
 * OUT_OF_SCOPE is both what the store accepts and what the banner says. A
 * verdict from an older build carrying some other value lands the same way, for
 * the reason `bannerKey` does: claim least.
 */
export function modelRouteOf(guard) {
  const route = guard && guard.route;
  return isContractRoute(route) ? route : 'OUT_OF_SCOPE';
}

/* ---------------- the removal receipt ---------------- */

export const CRISIS_ADDED_ROW = 'The product\'s crisis line was added under the reply.';
export const TIMEFRAME_WITHHELD_ROW =
  'A stated time frame could not be removed, so the model\'s own text is not shown.';
export const UNCLEAR_LINE_ROW = 'The model stated no disposition, so the product\'s line was added.';
export const NO_CHANGES_ROW = 'Nothing was removed from this reply.';

/**
 * What the guard did to this reply, in rows a person can read.
 *
 * THE POINT OF SHOWING IT AT ALL: every one of these changes is invisible on
 * screen. A removed sentence leaves no gap, a stripped time frame leaves a
 * tidied sentence, an appended crisis block reads as part of the reply, and the
 * time-frame fallback replaces the model's words with a fixed note that says
 * nothing about what it replaced. The health worker is being asked to confirm a
 * route for a reply they are only seeing part of — so what they are not seeing
 * is listed, verbatim and in the order it was written.
 *
 * Removals first, then additions: the receipt reads as "this came out, then this
 * went in", which is the order it happened in `applyGuard`.
 *
 * `verdict.prohibited` (the phrases the detectors objected to) is deliberately
 * NOT here. It is the REASON, not the receipt, and the two answer different
 * questions — a phrase listed there can still be on screen. The controller's
 * amendment asks for the receipt alone.
 */
export function receiptRows(guard) {
  if (!guard) return [];
  const rows = [];
  for (const sentence of guard.prohibitedRemoved ?? []) rows.push(`Removed: ${sentence}`);
  for (const phrase of guard.timeframeStripped ?? []) rows.push(`Time frame removed: ${phrase}`);
  if (guard.timeframeUnlocated) rows.push(TIMEFRAME_WITHHELD_ROW);
  if (guard.crisisLineAppended) rows.push(CRISIS_ADDED_ROW);
  if (guard.route === 'UNCLEAR') rows.push(UNCLEAR_LINE_ROW);
  return rows;
}

/**
 * The disclosure toggle's text, in `renderCitations`/`renderCalculations`'
 * grammar — same chevron, same "N things" shape, because it is the same kind of
 * collapsed provenance panel under the same kind of bubble.
 *
 * A count of zero is stated rather than hidden: "the guard changed nothing" is
 * information a person confirming a route wants, and an absent panel would be
 * indistinguishable from a panel that failed to render.
 */
export function receiptLabel(count, open = false) {
  const chevron = open ? '⌃' : '⌄';
  if (!count) return `${chevron} Reply shown unchanged`;
  return `${chevron} ${count} change${count === 1 ? '' : 's'} to the reply`;
}

/* ---------------- the confirmation itself ---------------- */

/**
 * What the confirmed line says.
 *
 * An override says both routes. The export's `overridden` flag is computed in
 * the store from exactly these two values, and the screen must not be the one
 * place they are conflated into "confirmed".
 */
export function confirmStatusText({ confirmedRoute = null, modelRoute = null } = {}) {
  if (!confirmedRoute) return null;
  return confirmedRoute === modelRoute
    ? `Confirmed: ${routeLabel(confirmedRoute)}`
    : `Changed to: ${routeLabel(confirmedRoute)}. The model said ${routeLabel(modelRoute)}.`;
}

/** What an unsupervised chat gets: nothing, and the same nothing every time. */
export const NO_CONFIRM_STATE = Object.freeze({
  render: false,
  controls: 'none',
  modelRoute: null,
  options: Object.freeze([]),
  confirmedRoute: null,
  confirmedAt: null,
  statusText: null,
  receipt: Object.freeze({ count: 0, rows: Object.freeze([]) }),
});

/**
 * Everything the confirmation block draws, for one message.
 *
 * `controls` is the whole state machine:
 *   'active'  a verdict, a persisted row id, no confirmation yet;
 *   'locked'  a confirmation is recorded — the buttons stay on screen and are
 *             disabled, so what was chosen is still legible rather than gone;
 *   'none'    nothing to write against. THE ROW THIS BRANCH EXISTS FOR is a
 *             reply whose persist failed: `attach_guard`/`append_message` never
 *             returned an id, so a confirmation has no row. The RECEIPT is
 *             still drawn — the guard changed what is on screen either way, and
 *             withholding the receipt too would hide the removal AND the reason.
 *
 * `supervised` must be exactly `true`, Task 6's rule: this is asked of the
 * CHAT's entry, and a truthy value that is not the flag is not an answer.
 */
export function confirmState({ supervised = false, message = null } = {}) {
  const msg = message || {};
  const guard = msg.guard;
  if (supervised !== true || !guard) return NO_CONFIRM_STATE;

  const modelRoute = modelRouteOf(guard);
  const confirmedRoute = msg.confirmedRoute ?? null;
  const confirmedAt = msg.confirmedAt ?? null;
  // A locked picker shows what the health worker chose, not what the model
  // said: the select is the record of the decision once one exists.
  const selected = confirmedRoute ?? modelRoute;
  const rows = receiptRows(guard);

  return {
    render: true,
    controls: confirmedRoute ? 'locked' : (msg.id == null ? 'none' : 'active'),
    modelRoute,
    options: CONFIRM_ROUTES.map((value) => ({
      value,
      label: ROUTE_LABELS[value],
      selected: value === selected,
    })),
    confirmedRoute,
    confirmedAt,
    statusText: confirmStatusText({ confirmedRoute, modelRoute }),
    receipt: { count: rows.length, rows: rows.length ? rows : [NO_CHANGES_ROW] },
  };
}

/**
 * The invoke for one confirmation, or null when there is nothing to send.
 *
 * Refused HERE as well as in the store, and not as belt-and-braces: an
 * off-contract route reaching `confirm_route` comes back as a rejected promise,
 * which this app's shape would surface to the health worker as an engine banner
 * about a route they never typed. The picker cannot produce one; a replayed
 * verdict from an older build could.
 */
export function confirmRequest({ message = null, route = null } = {}) {
  const id = message && message.id;
  if (id == null || !isContractRoute(route)) return null;
  return { command: 'confirm_route', args: { messageId: id, route } };
}

/**
 * What was recorded, from whatever `confirm_route` handed back.
 *
 * TODAY IT HANDS BACK NOTHING. `convstore::confirm_route` is
 * `Result<(), String>` (convstore.rs:1859), so the resolve value over IPC is
 * `null`. It RESOLVED, which is the store saying it took the route — so the
 * route the health worker chose is what the screen reflects, and `confirmedAt`
 * (the store's own clock) stays unknown until the chat is reopened, which is
 * also the only place the export ever reads it from.
 *
 * If that command is ever changed to return the row — the `MessageInfo` shape
 * `attach_guard` already returns, camelCase `confirmedRoute`/`confirmedAt` — the
 * store's own values win here with no second change anywhere.
 */
export function confirmResult(result, requestedRoute) {
  const info = result && typeof result === 'object' ? result : null;
  return {
    confirmedRoute: (info && info.confirmedRoute) || requestedRoute || null,
    confirmedAt: (info && info.confirmedAt) || null,
  };
}

/* ---------------- the triage-log export ---------------- */

export const TRIAGE_EXPORT_LABEL = 'Triage log (JSONL)';
export const TRIAGE_EXPORT_UNAVAILABLE =
  'The triage log can only be exported from the desktop app.';

/** `export_triage_log(user, None)` walks every chat; the name says which. */
export function triageLogFileName(chatId) {
  return `triage-log-${chatId == null ? 'all' : chatId}.jsonl`;
}

/**
 * How this chat's triage log gets out, if it can.
 *
 *   'none'         not a supervised chat. The menu entry does not exist.
 *   'save'         desktop: the OS save sheet, then Task 7's command.
 *   'unavailable'  a supervised chat on a phone.
 *
 * WHY MOBILE HAS NO DESTINATION, stated here so it is in one place rather than
 * discovered again: `exportChat`'s mobile branch goes to `share_chat`, which
 * formats through `convstore::export_chat` and accepts markdown/json/txt only —
 * it cannot carry this file. And `dialog.save` on Android is
 * `ACTION_CREATE_DOCUMENT`, which hands back a `content://` URI that
 * `export_triage_log_to_file`'s `std::fs::write` cannot write to. Both fixes are
 * Rust (a `share_triage_log` beside `share_chat`), so until one exists the entry
 * is not offered and the reason is not a mystery. Flip `offersTriageExport` to
 * accept 'unavailable' — or make this return 'share' — the day it lands.
 */
export function triageExportPlan({ supervised = false, isMobile = false, chatId = null } = {}) {
  if (supervised !== true) return { kind: 'none', message: null };
  if (isMobile) return { kind: 'unavailable', message: TRIAGE_EXPORT_UNAVAILABLE };
  return {
    kind: 'save',
    message: null,
    command: 'export_triage_log_to_file',
    // ONLY the chat id. `path` is added at call time from the save sheet; the
    // bytes are `export_triage_log`'s own and the front end reshapes none of
    // them.
    args: { chatId: chatId ?? null },
    defaultPath: triageLogFileName(chatId),
    filters: [{ name: 'JSON Lines', extensions: ['jsonl'] }],
  };
}

/** Whether the export menu shows the entry at all. */
export function offersTriageExport({ supervised = false, isMobile = false } = {}) {
  return triageExportPlan({ supervised, isMobile }).kind === 'save';
}
