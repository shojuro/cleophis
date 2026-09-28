// src/triage-confirm.test.mjs — node --test src/
//
// Task 8's decisions, pulled out of `app.js` for the same reason Task 6's were:
// `app.js` reaches for `window.__TAURI__` at module scope and cannot be
// imported here, so anything worth arguing about lives in `triage-confirm.js`
// and `app.js` keeps only the DOM writes around it.
//
// What is worth arguing about here:
//   1. which routes the picker may offer, and what an off-contract route does;
//   2. when the Confirm/Change controls exist at all, and when they are locked;
//   3. what a reply's removal RECEIPT says — the sentences the guard took out,
//      quoted as written;
//   4. what the front end does with `confirm_route`'s return value, which today
//      is nothing;
//   5. whether the triage-log export is offered, and with what arguments.
//
// The load-bearing block is TUTOR IDENTITY: an unsupervised chat gets no
// confirmation UI and no triage-log menu entry, even when a row in it somehow
// carries a verdict.
import { test } from 'node:test';
import assert from 'node:assert';
import { applyGuard, carriesDose, redactDoses } from './triage/guard.js';
import {
  CONFIRM_ROUTES, CRISIS_ADDED_ROW, CRISIS_LEADS_ROW, CRISIS_ONLY_ROW, CRISIS_REPLACED_ROW, NO_CHANGES_ROW, NO_CONFIRM_STATE, ROUTE_LABELS,
  signpostsRemovedRow, dosesRemovedRow, DISCLAIMER_REMOVED_PREFIX,
  TIMEFRAME_WITHHELD_ROW, TRIAGE_EXPORT_LABEL, TRIAGE_EXPORT_UNAVAILABLE, UNCLEAR_LINE_ROW,
  confirmRequest, confirmResult, confirmState, confirmStatusText, modelRouteOf,
  offersTriageExport, receiptLabel, receiptRows, routeLabel, triageExportPlan, triageLogFileName,
} from './triage-confirm.js';

/** A guard verdict shaped like `applyGuard`'s, with nothing removed. */
function verdict(over = {}) {
  return {
    route: 'CLINICIAN',
    why: 'test',
    banner: 'clinician',
    displayText: 'Have this looked at.',
    rawReply: 'Have this looked at.',
    timeframeStripped: [],
    timeframeUnlocated: false,
    crisisOnInput: false,
    crisisLineAppended: false,
    prohibited: { medication: [], diagnosis: [] },
    prohibitedRemoved: [],
    detectorsSha: 'deadbeef',
    ...over,
  };
}

/* ---------------- which routes the picker may offer ---------------- */

test('the four contract routes are the four the confirm command accepts', () => {
  // `convstore::ConvStore::confirm_route`'s ROUTES, in its order. A fifth entry
  // here would be a route the front end can offer and the store must refuse.
  assert.deepStrictEqual(CONFIRM_ROUTES, ['EMERGENCY', 'CLINICIAN', 'SELF_CARE', 'OUT_OF_SCOPE']);
  assert.deepStrictEqual(Object.keys(ROUTE_LABELS), CONFIRM_ROUTES);
});

test('each contract route is offered as itself', () => {
  for (const route of CONFIRM_ROUTES) {
    assert.strictEqual(modelRouteOf({ route }), route);
  }
});

test('UNCLEAR is offered as OUT_OF_SCOPE, the route that claims least', () => {
  // UNCLEAR is a detector answer, not a disposition, and `confirm_route`
  // refuses it. The banner already reads CANNOT JUDGE for it (ROUTE_TO_BANNER),
  // so the button pre-selects the route that matches what is on screen.
  assert.strictEqual(modelRouteOf({ route: 'UNCLEAR' }), 'OUT_OF_SCOPE');
});

test('a verdict from an older build with no route it knows offers OUT_OF_SCOPE', () => {
  assert.strictEqual(modelRouteOf({ route: 'ROUTE_X' }), 'OUT_OF_SCOPE');
  assert.strictEqual(modelRouteOf({}), 'OUT_OF_SCOPE');
  assert.strictEqual(modelRouteOf(null), 'OUT_OF_SCOPE');
});

test('a stored route this build does not know is shown as itself, never relabelled', () => {
  // The two questions are different. WHICH route to offer normalises toward the
  // route that claims least; what a health worker's RECORDED decision SAYS does
  // not — printing "Cannot judge" over a stored EMERGENCY_2 would misreport the
  // one thing on screen that is a person's own act.
  assert.strictEqual(routeLabel('EMERGENCY'), 'Emergency');
  assert.strictEqual(routeLabel('ROUTE_X'), 'ROUTE_X');
});

/* ---------------- when the controls exist, and when they lock ------------- */

test('a supervised reply with a persisted row id offers the controls', () => {
  const st = confirmState({ supervised: true, message: { id: 7, guard: verdict() } });
  assert.strictEqual(st.render, true);
  assert.strictEqual(st.controls, 'active');
  assert.strictEqual(st.modelRoute, 'CLINICIAN');
  assert.strictEqual(st.confirmedRoute, null);
  assert.strictEqual(st.statusText, null);
});

test('the picker offers the four routes with the model\'s route pre-selected', () => {
  const st = confirmState({ supervised: true, message: { id: 7, guard: verdict({ route: 'SELF_CARE' }) } });
  assert.deepStrictEqual(st.options, [
    { value: 'EMERGENCY', label: 'Emergency', selected: false },
    { value: 'CLINICIAN', label: 'See a clinician', selected: false },
    { value: 'SELF_CARE', label: 'Self-care', selected: true },
    { value: 'OUT_OF_SCOPE', label: 'Cannot judge', selected: false },
  ]);
});

test('a reply whose persist never landed shows the receipt and no controls', () => {
  // THE ROW THIS BRANCH EXISTS FOR: `attach_guard`/`append_message` failed, so
  // there is no row id and a confirmation has nothing to be written against.
  // The guard still changed what is on screen, and the receipt still says how —
  // withholding the receipt as well would hide the removal AND the reason.
  const st = confirmState({ supervised: true, message: { guard: verdict({ prohibitedRemoved: ['Take 400mg.'] }) } });
  assert.strictEqual(st.render, true);
  assert.strictEqual(st.controls, 'none');
  assert.deepStrictEqual(st.receipt.rows, ['Removed: Take [dose removed].']);
});

test('a confirmed reply comes back locked, naming the confirmed route', () => {
  const st = confirmState({
    supervised: true,
    message: { id: 7, guard: verdict(), confirmedRoute: 'CLINICIAN', confirmedAt: '2026-09-10T11:00:00+01:00' },
  });
  assert.strictEqual(st.controls, 'locked');
  assert.strictEqual(st.confirmedRoute, 'CLINICIAN');
  assert.strictEqual(st.confirmedAt, '2026-09-10T11:00:00+01:00');
  assert.strictEqual(st.statusText, 'Confirmed: See a clinician');
});

test('a locked picker shows the CONFIRMED route, not the model\'s', () => {
  const st = confirmState({
    supervised: true,
    message: { id: 7, guard: verdict({ route: 'SELF_CARE' }), confirmedRoute: 'EMERGENCY' },
  });
  assert.deepStrictEqual(st.options.filter((o) => o.selected), [
    { value: 'EMERGENCY', label: 'Emergency', selected: true },
  ]);
});

test('an override says so, and says what the model said', () => {
  // The export's `overridden` flag is computed in the store from the same two
  // values; the screen must not be the one place they are conflated.
  assert.strictEqual(
    confirmStatusText({ confirmedRoute: 'EMERGENCY', modelRoute: 'SELF_CARE' }),
    'Changed to: Emergency. The model said Self-care.',
  );
  assert.strictEqual(confirmStatusText({ confirmedRoute: null, modelRoute: 'SELF_CARE' }), null);
});

test('an assistant row with no verdict has nothing to confirm', () => {
  // Task 6 withholds this row's text behind the `unverified` banner. There is
  // no route on it, so there is no route to confirm either.
  const st = confirmState({ supervised: true, message: { id: 7, role: 'assistant', content: 'x' } });
  assert.deepStrictEqual(st, NO_CONFIRM_STATE);
});

/* ---------------- TUTOR IDENTITY ---------------- */

test('an unsupervised chat renders nothing, even for a row carrying a verdict', () => {
  // The stray-verdict case is real: `entryForChat` exists because the open chat
  // and the entered model routinely disagree, and a tutor chat holding one
  // triage row must not sprout a clinical confirmation control.
  const st = confirmState({ supervised: false, message: { id: 7, guard: verdict() } });
  assert.deepStrictEqual(st, NO_CONFIRM_STATE);
  assert.strictEqual(st.render, false);
});

test('supervised must be exactly true — a truthy value does not arm the UI', () => {
  for (const supervised of [1, 'yes', {}, undefined, null]) {
    assert.deepStrictEqual(confirmState({ supervised, message: { id: 7, guard: verdict() } }), NO_CONFIRM_STATE);
  }
});

test('an unsupervised chat is offered no triage-log export, on either platform', () => {
  // Now that mobile HAS a destination, the supervised check is the only thing
  // keeping an ordinary chat's menu clean — assert it on both platforms.
  for (const isMobile of [false, true]) {
    assert.strictEqual(triageExportPlan({ supervised: false, isMobile, chatId: 3 }).kind, 'none');
    assert.strictEqual(offersTriageExport({ supervised: false, isMobile, chatId: 3 }), false);
    for (const supervised of [1, 'yes', undefined, null]) {
      assert.strictEqual(offersTriageExport({ supervised, isMobile, chatId: 3 }), false);
    }
  }
});

/* ---------------- the removal receipt ---------------- */

test('every removed sentence is quoted as written, in the order it was written', () => {
  const rows = receiptRows(verdict({
    prohibitedRemoved: ['Take ibuprofen 400mg every six hours.', 'This is likely appendicitis.'],
  }));
  // Whole-branch fix round, I5: the dose in a removed sentence is redacted in
  // the receipt too; a diagnosis sentence stays verbatim.
  assert.deepStrictEqual(rows, [
    'Removed: Take ibuprofen [dose removed] every six hours.',
    'Removed: This is likely appendicitis.',
  ]);
});

test('a stripped time frame is listed as written', () => {
  const rows = receiptRows(verdict({ timeframeStripped: ['within 2 days', 'today'] }));
  assert.deepStrictEqual(rows, [
    'Time frame removed: within 2 days',
    'Time frame removed: today',
  ]);
});

test('what the guard ADDED is on the receipt too, after what it removed', () => {
  const rows = receiptRows(verdict({
    prohibitedRemoved: ['Take 400mg.'],
    crisisLineAppended: true,
  }));
  assert.deepStrictEqual(rows, ['Removed: Take [dose removed].', CRISIS_ADDED_ROW]);
});

test('a withheld referral says the model\'s text is not on screen', () => {
  // `timeframeUnlocated` is the one flag whose consequence a reader cannot see:
  // the bubble holds the fixed note and nothing says the model wrote more.
  const rows = receiptRows(verdict({ timeframeUnlocated: true }));
  assert.deepStrictEqual(rows, [TIMEFRAME_WITHHELD_ROW]);
});

test('an UNCLEAR reply says the product added the line under it', () => {
  const rows = receiptRows(verdict({ route: 'UNCLEAR' }));
  assert.deepStrictEqual(rows, [UNCLEAR_LINE_ROW]);
});

test('a reply the guard left alone says so rather than showing an empty list', () => {
  const st = confirmState({ supervised: true, message: { id: 7, guard: verdict() } });
  assert.strictEqual(st.receipt.count, 0);
  assert.deepStrictEqual(st.receipt.rows, [NO_CHANGES_ROW]);
  assert.strictEqual(receiptLabel(0, false), '⌄ Reply shown unchanged');
});

test('the toggle counts the changes and flips its chevron', () => {
  assert.strictEqual(receiptLabel(1, false), '⌄ 1 change to the reply');
  assert.strictEqual(receiptLabel(3, false), '⌄ 3 changes to the reply');
  assert.strictEqual(receiptLabel(3, true), '⌃ 3 changes to the reply');
});

test('the receipt of a REAL verdict names the sentence the guard removed', () => {
  // Against `applyGuard` rather than a hand-written verdict, so the receipt
  // cannot drift from the shape the guard actually produces.
  const v = applyGuard({
    userText: 'My child has a fever.',
    replyText: 'This sounds like a viral illness. Give ibuprofen 400mg every six hours. See your GP.',
  });
  assert.ok(v.prohibitedRemoved.length > 0, 'the fixture must actually trip the filter');
  const rows = receiptRows(v);
  for (const sentence of v.prohibitedRemoved) {
    const listed = carriesDose(sentence) ? redactDoses(sentence) : sentence;
    assert.ok(rows.includes(`Removed: ${listed}`), `receipt is missing: ${sentence}`);
  }
  assert.ok(rows.every((r) => !carriesDose(r)), 'no receipt row repeats a dose');
  assert.ok(!v.displayText.includes('400mg'), 'the dose must not be on screen');
});

/* ---------------- writing the confirmation ---------------- */

test('the confirm request is Task 7\'s command with its camelCase arguments', () => {
  assert.deepStrictEqual(
    confirmRequest({ message: { id: 42, guard: verdict() }, route: 'EMERGENCY' }),
    { command: 'confirm_route', args: { messageId: 42, route: 'EMERGENCY' } },
  );
});

test('a route the store would refuse is refused before it is sent', () => {
  const msg = { id: 42, guard: verdict() };
  for (const route of ['UNCLEAR', 'MAYBE', '', null, undefined]) {
    assert.strictEqual(confirmRequest({ message: msg, route }), null);
  }
});

test('a reply with no persisted row id sends nothing', () => {
  assert.strictEqual(confirmRequest({ message: { guard: verdict() }, route: 'EMERGENCY' }), null);
  assert.strictEqual(confirmRequest({ message: null, route: 'EMERGENCY' }), null);
});

test('a resolve value of null still reflects the route that was accepted', () => {
  // Not the normal path since P2.5 — but a confirmation that RESOLVED did
  // record the route, so a caller with no row to read still reflects it, and
  // `confirmedAt` (the store's clock) is honestly unknown rather than invented.
  assert.deepStrictEqual(confirmResult(null, 'EMERGENCY'), { confirmedRoute: 'EMERGENCY', confirmedAt: null });
  assert.deepStrictEqual(confirmResult(undefined, 'SELF_CARE'), { confirmedRoute: 'SELF_CARE', confirmedAt: null });
});

test('the MessageInfo confirm_route now returns wins over the route that was asked for', () => {
  // `convstore::confirm_route` returns the updated row (the shape
  // `attach_guard` already returned). The store's own values are the truth —
  // `confirmedAt` above all, which the front end cannot compute — so the banner
  // is right without reopening the chat.
  assert.deepStrictEqual(
    confirmResult({ id: 42, confirmedRoute: 'CLINICIAN', confirmedAt: '2026-09-10T11:00:00+01:00' }, 'EMERGENCY'),
    { confirmedRoute: 'CLINICIAN', confirmedAt: '2026-09-10T11:00:00+01:00' },
  );
});

/* ---------------- the triage-log export ---------------- */

test('a supervised chat on desktop is exported through Task 7\'s command', () => {
  const plan = triageExportPlan({ supervised: true, isMobile: false, chatId: 12 });
  assert.strictEqual(plan.kind, 'save');
  assert.strictEqual(plan.command, 'export_triage_log_to_file');
  // ONLY the chat id. The bytes are `export_triage_log`'s own; the front end
  // picks a destination and reshapes nothing.
  assert.deepStrictEqual(plan.args, { chatId: 12 });
  assert.strictEqual(plan.defaultPath, 'triage-log-12.jsonl');
  assert.deepStrictEqual(plan.filters, [{ name: 'JSON Lines', extensions: ['jsonl'] }]);
  assert.strictEqual(offersTriageExport({ supervised: true, isMobile: false }), true);
});

test('a chat id of null exports every chat of the account', () => {
  // `export_triage_log(user, None)` walks `list_chats`; the file name says so.
  assert.strictEqual(triageLogFileName(null), 'triage-log-all.jsonl');
  assert.deepStrictEqual(triageExportPlan({ supervised: true, isMobile: false }).args, { chatId: null });
});

test('a supervised chat on Android is shared through share_triage_log', () => {
  // The phone's destination is the share sheet, not a file picker. `share_chat`
  // could not carry this file (it formats through `convstore::export_chat` —
  // markdown/json/txt), and `dialog.save` on Android hands back a `content://`
  // URI that `export_triage_log_to_file`'s `std::fs::write` cannot write to.
  // `share_triage_log` closed both gaps.
  const plan = triageExportPlan({ supervised: true, isMobile: true, chatId: 12, title: 'Chest pain' });
  assert.strictEqual(plan.kind, 'share');
  assert.strictEqual(plan.command, 'share_triage_log');
  assert.strictEqual(plan.message, null);
  // The chat id and the chooser's title, nothing else — no path, because Rust
  // launches the chooser, and no format, because there is only one.
  assert.deepStrictEqual(plan.args, { chatId: 12, title: 'Chest pain' });
  assert.strictEqual(offersTriageExport({ supervised: true, isMobile: true, chatId: 12 }), true);
});

test('an untitled chat still shares under a usable name', () => {
  // Rust sanitises the title into the filename (`safe_file_stem`), but an empty
  // string sanitises to the same fallback the chooser would show as blank — so
  // the seam supplies one rather than shipping an empty chooser label.
  for (const title of ['', null, undefined]) {
    assert.deepStrictEqual(
      triageExportPlan({ supervised: true, isMobile: true, chatId: 3, title }).args,
      { chatId: 3, title: 'chat' },
    );
  }
});

test('exporting every chat at once stays desktop-only, and says why', () => {
  // `share_triage_log`'s `chat_id` is an `i64`, deliberately not an `Option`:
  // a chooser must not be able to hand every supervised conversation the
  // account has to whatever app is tapped next. Desktop keeps the all-chats
  // export because the reviewer picks a file and can inspect it first.
  const plan = triageExportPlan({ supervised: true, isMobile: true, chatId: null, title: 'x' });
  assert.strictEqual(plan.kind, 'unavailable');
  assert.strictEqual(plan.message, TRIAGE_EXPORT_UNAVAILABLE);
  assert.strictEqual(offersTriageExport({ supervised: true, isMobile: true, chatId: null }), false);
  // ...and it is refused HERE, never sent on to fail as an IPC type error.
  assert.strictEqual(plan.command, undefined);
});

test('the menu entry names the file format it writes', () => {
  assert.strictEqual(TRIAGE_EXPORT_LABEL, 'Triage log (JSONL)');
});

/* ---------------- Phase 1h M2: the replace rule on the receipt ---------------- */

test('a reply REPLACED by the crisis block says so, and lists nothing about text that is not shown', () => {
  const v = applyGuard({
    userText: "i don't want to be here anymore",
    replyText: 'Hmm. Take 400 mg ibuprofen every six hours.',
    crisisRule: 'replace',
  });
  assert.strictEqual(v.replyShown, false);
  assert.deepStrictEqual(receiptRows(v), [CRISIS_REPLACED_ROW]);
});

test('a reply KEPT under the crisis block keeps its removals, and says the block leads', () => {
  const v = applyGuard({
    userText: "i don't want to be here anymore",
    replyText: 'Call 999 now for an ambulance. Take 400 mg ibuprofen every six hours.',
    crisisRule: 'replace',
  });
  assert.strictEqual(v.replyShown, true);
  const rows = receiptRows(v);
  assert.ok(rows.length >= 2, rows.join(' | '));
  assert.strictEqual(rows[rows.length - 1], CRISIS_LEADS_ROW);
  assert.ok(rows.slice(0, -1).every((r) => r.startsWith('Removed: ')), rows.join(' | '));
  assert.strictEqual(rows.includes(CRISIS_ADDED_ROW), false, 'nothing was added UNDER the reply');
});

/* ---------------- Phase 1i MA1: de-duplicated signposts on the receipt ---------------- */

test('a kept reply that lost a crisis-line sentence says so by COUNT, never by the sentence', () => {
  const v = applyGuard({
    userText: "i don't want to be here anymore",
    replyText: 'Call 999 now for an ambulance. You can also ring the Samaritans on 116 124.',
    crisisRule: 'replace',
  });
  assert.strictEqual(v.signpostsRemoved, 1);
  const rows = receiptRows(v);
  assert.deepStrictEqual(rows, [signpostsRemovedRow(1), CRISIS_LEADS_ROW]);
  assert.strictEqual(rows.join(' ').includes('116 124'), false, 'the wrong number is not in the receipt');
  assert.match(signpostsRemovedRow(2), /^2 sentences/);
});

test('a kept reply de-duplication EMPTIED gets its own rows, never "replaced" (whole-branch fix round, M4)', () => {
  const v = applyGuard({
    userText: "i don't want to be here anymore",
    replyText: 'Go to A&E now and call the Samaritans on 116 123.',
    crisisRule: 'replace',
  });
  assert.strictEqual(v.replyShown, false);
  assert.strictEqual(v.banner, 'emergency');
  assert.deepStrictEqual(receiptRows(v), [signpostsRemovedRow(1), CRISIS_ONLY_ROW]);
});

/* ---------------- Phase 1h M6: a lookup row has no route to confirm ---------------- */

test('a lookup reply or query row draws no confirmation controls and no receipt', () => {
  for (const guard of [
    { kind: 'lookup', rule: 'dose-cite-v1', outcome: 'grounded', displayText: 'x [1].', withheld: [], citations: [1] },
    { kind: 'lookup', role: 'query' },
  ]) {
    assert.deepStrictEqual(confirmState({ supervised: true, message: { id: 7, guard } }), NO_CONFIRM_STATE);
  }
});

/* ---------------- Phase 1i MA6: doses and disclaimers on the receipt ---------------- */

test('a dose cut says so by COUNT, never by the dose', () => {
  const v = applyGuard({ userText: 'my head hurts', replyText: 'Do not take more than 8 tablets in 24 hours. Please see your GP.' });
  assert.strictEqual(v.dosesRemoved, 1);
  const rows = receiptRows(v);
  assert.deepStrictEqual(rows, [dosesRemovedRow(1)]);
  assert.strictEqual(dosesRemovedRow(1), 'One sentence giving a dose was removed; a clinician can advise on treatment.');
  assert.match(dosesRemovedRow(2), /^2 sentences giving a dose were removed/);
  assert.strictEqual(rows.join(' ').includes('8 tablets'), false, 'the dose is not in the receipt');
});

test('a redacted dose in a route-bearing sentence is counted the same way', () => {
  const v = applyGuard({ userText: 'asthma', replyText: 'Give 2 puffs of the blue inhaler and call 999 if no better in 5 minutes.' });
  assert.strictEqual(v.dosesRemoved, 1);
  assert.match(v.displayText, /\[dose removed\]/);
  const rows = receiptRows(v);
  assert.ok(rows.includes(dosesRemovedRow(v.dosesRemoved)), rows.join(' | '));
  assert.strictEqual(/\d puffs/.test(rows.join(' ')), false);
});

test('a removed scope disclaimer is listed verbatim', () => {
  const v = applyGuard({ userText: 'lump in my neck', replyText: 'I cannot assess you properly. Please see your GP.' });
  assert.deepStrictEqual(receiptRows(v), [`${DISCLAIMER_REMOVED_PREFIX}I cannot assess you properly.`]);
});

test('MA6 rows sit after prohibitedRemoved and before the signpost row, doses before disclaimers', () => {
  const rows = receiptRows(verdict({
    prohibitedRemoved: ['Take ibuprofen.'], dosesRemoved: 2, disclaimersRemoved: ['I cannot assess you.'], signpostsRemoved: 1,
  }));
  assert.deepStrictEqual(rows, [
    'Removed: Take ibuprofen.', dosesRemovedRow(2), `${DISCLAIMER_REMOVED_PREFIX}I cannot assess you.`, signpostsRemovedRow(1),
  ]);
});

test('an older verdict with no MA6 keys, or zero counts, adds no rows', () => {
  assert.deepStrictEqual(receiptRows(verdict({})), []);
  assert.deepStrictEqual(receiptRows(verdict({ dosesRemoved: 0, disclaimersRemoved: [] })), []);
});
