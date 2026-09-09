// src/triage-turn.test.mjs — node --test src/
//
// The decisions Task 6 makes on the send path, pulled out of `app.js` so they
// can be run: which banner, when the provisional one appears, how the turn is
// persisted, and what titles the chat. `app.js` itself reaches for
// `window.__TAURI__` at module scope and cannot be imported here — so every
// branch worth arguing about lives in this module, and `app.js` is left with
// the DOM writes that surround it.
//
// The load-bearing test in this file is the TUTOR IDENTITY block: the tutor is
// not supervised, and its turns must reach exactly the invokes they reached
// before this task, with exactly the same arguments.
import { test } from 'node:test';
import assert from 'node:assert';
import { BANNERS } from './triage/guard.js';
import { bannerKey, bannerText, persistAssistantTurn, provisionalStep, supervisedTitle, titlePlan } from './triage-turn.js';

/* ---------------- the banner shown above a reply ---------------- */

test('bannerText returns the product copy for each of the four banners', () => {
  for (const key of ['emergency', 'clinician', 'self_care', 'out_of_scope']) {
    assert.deepStrictEqual(bannerText(key), BANNERS[key]);
  }
});

test('an unknown banner key renders as CANNOT JUDGE rather than as nothing', () => {
  // A blank banner on a triage reply is the one rendering failure that reads
  // as "no disposition"; fail toward the banner that claims least.
  assert.deepStrictEqual(bannerText('nonsense'), BANNERS.out_of_scope);
  assert.deepStrictEqual(bannerText(undefined), BANNERS.out_of_scope);
});

test('the class and the copy are built from the same normalised key', () => {
  // `triage-banner--<key>` is what colours the banner. If the key that picks
  // the copy and the key that picks the colour could differ, an unknown key
  // would render CANNOT JUDGE copy on no background at all.
  assert.strictEqual(bannerKey('emergency'), 'emergency');
  assert.strictEqual(bannerKey('nonsense'), 'out_of_scope');
  assert.strictEqual(bannerKey(undefined), 'out_of_scope');
  // Inherited names are not banner keys.
  assert.strictEqual(bannerKey('constructor'), 'out_of_scope');
  assert.strictEqual(bannerKey('toString'), 'out_of_scope');
});

/* ---------------- the provisional banner, mid-stream ---------------- */

test('a supervised prefix that already states a disposition shows it provisionally, and logs once', () => {
  const step = provisionalStep({
    supervised: true,
    alreadyShown: false,
    prefixText: 'Call an ambulance now. This needs emergency care immediately.',
    elapsedMs: 812.4,
  });
  assert.strictEqual(step.route, 'EMERGENCY');
  assert.strictEqual(step.banner, 'emergency');
  assert.strictEqual(step.log, '[triage] time-to-route 812 ms');
});

test('a prefix the detectors cannot route yet shows nothing and logs nothing', () => {
  const step = provisionalStep({ supervised: true, alreadyShown: false, prefixText: 'Thank you for', elapsedMs: 40 });
  assert.deepStrictEqual(step, { route: null, banner: null, log: null });
});

test('an empty prefix shows nothing', () => {
  assert.deepStrictEqual(
    provisionalStep({ supervised: true, alreadyShown: false, prefixText: '', elapsedMs: 0 }),
    { route: null, banner: null, log: null },
  );
});

test('the provisional banner resolves ONCE — a later delta never re-logs the time to route', () => {
  // `[triage] time-to-route` is the device session's own measurement (Phase 3).
  // A line per token would make it useless, and a second line for the same turn
  // would be a different measurement wearing the same name.
  const first = provisionalStep({ supervised: true, alreadyShown: false, prefixText: 'See a clinician about this.', elapsedMs: 300 });
  assert.ok(first.log);
  const later = provisionalStep({ supervised: true, alreadyShown: true, prefixText: 'See a clinician about this, and also…', elapsedMs: 900 });
  assert.deepStrictEqual(later, { route: null, banner: null, log: null });
});

test('an OUT_OF_SCOPE prefix is a disposition and does show provisionally', () => {
  // OUT_OF_SCOPE is the model declining ON THE RECORD — R14's shape, a scope
  // disclaimer plus a signpost. UNCLEAR is the detectors not being able to read
  // a disposition yet. Only the second is "keep waiting".
  const step = provisionalStep({
    supervised: true,
    alreadyShown: false,
    prefixText: 'I have no basis to triage this. Please see your GP.',
    elapsedMs: 100,
  });
  assert.strictEqual(step.banner, 'out_of_scope');
  assert.ok(step.route && step.route !== 'UNCLEAR');
});

/* ---------------- persisting the assistant turn ---------------- */

test('a supervised turn with a row id ATTACHES the verdict — it never appends a second row', () => {
  // On mobile the checkpointer's row is finalized by `settle` BEFORE `Done`, so
  // an `append_message` here would write a SECOND assistant row for one turn.
  const guard = { route: 'EMERGENCY', banner: 'emergency', displayText: 'Go now.', rawReply: 'Go now.' };
  const plan = persistAssistantTurn({ chatId: 4, content: 'Go now.', guard, messageId: 77, calculations: [] });
  assert.strictEqual(plan.command, 'attach_guard');
  assert.deepStrictEqual(plan.args, { messageId: 77, guard });
});

test('a supervised turn with no row id (the desktop path) appends WITH the verdict', () => {
  const guard = { route: 'SELF_CARE', banner: 'self_care', displayText: 'Rest.', rawReply: 'Rest.' };
  const plan = persistAssistantTurn({ chatId: 4, content: 'Rest.', guard, messageId: null });
  assert.strictEqual(plan.command, 'append_message');
  assert.deepStrictEqual(plan.args, {
    chatId: 4, role: 'assistant', content: 'Rest.', citations: null, toolCalls: null, guard,
  });
});

test('the appended content is the DISPLAY text, and the raw reply survives inside the verdict', () => {
  const guard = { route: 'CLINICIAN', banner: 'clinician', displayText: 'See a clinician.', rawReply: 'See a clinician within 2 days.' };
  const plan = persistAssistantTurn({ chatId: 1, content: guard.displayText, guard, messageId: null });
  assert.strictEqual(plan.args.content, 'See a clinician.');
  assert.strictEqual(plan.args.guard.rawReply, 'See a clinician within 2 days.');
});

test('citations and calculations ride along exactly as they did before', () => {
  const cites = [{ docTitle: 'd' }];
  const calcs = [{ expression: '1+1', display: '2' }];
  const plan = persistAssistantTurn({ chatId: 9, content: 'x', citations: cites, calculations: calcs });
  assert.deepStrictEqual(plan.args.citations, cites);
  assert.deepStrictEqual(plan.args.toolCalls, calcs);
});

test('an empty turn, or one whose chat was never created, persists nothing', () => {
  assert.strictEqual(persistAssistantTurn({ chatId: 4, content: '' }), null);
  assert.strictEqual(persistAssistantTurn({ chatId: null, content: 'x' }), null);
  assert.strictEqual(persistAssistantTurn({ chatId: null, content: '', guard: { banner: 'emergency' }, messageId: null }), null);
});

/* ---------------- the title of a supervised chat ---------------- */

test('a supervised chat is titled from the user\'s own first message, not by a second model call', () => {
  const plan = titlePlan({ supervised: true, source: 'my father has crushing chest pain going down his left arm and he is sweating' });
  assert.deepStrictEqual(plan, { kind: 'fixed', title: 'my father has crushing chest pain going down his' });
  assert.strictEqual(plan.title.length, 48);
});

test('the fixed title is single-spaced and trimmed before it is cut', () => {
  // A multi-line description must not be cut at the newline, and must not
  // arrive with the newline in it.
  assert.strictEqual(supervisedTitle('  he is\n\n  short of   breath  '), 'he is short of breath');
  assert.strictEqual(supervisedTitle(''), '');
  assert.strictEqual(supervisedTitle(null), '');
});

test('the cut never leaves half a character behind', () => {
  // 48 CODE POINTS, matching `auto_title_chat`'s `chars().take(80)`. A slice by
  // UTF-16 unit would split an emoji into a lone surrogate.
  const title = supervisedTitle(`${'🚑'.repeat(60)}`);
  assert.strictEqual([...title].length, 48);
  assert.ok(!/[\uD800-\uDFFF]/.test(title.replace(/[\uD800-\uDBFF][\uDC00-\uDFFF]/g, '')), 'a lone surrogate survived the cut');
});

test('a whitespace-only first message titles nothing rather than titling blank', () => {
  assert.deepStrictEqual(titlePlan({ supervised: true, source: '   \n  ' }), { kind: 'none' });
});

test('a turn that is not a first exchange titles nothing on either path', () => {
  assert.deepStrictEqual(titlePlan({ supervised: true, source: null }), { kind: 'none' });
  assert.deepStrictEqual(titlePlan({ supervised: false, source: null }), { kind: 'none' });
});

/* ---------------- TUTOR IDENTITY ---------------- */

test('TUTOR IDENTITY: an unguarded turn persists with exactly the invoke args it used before this task', () => {
  // The tutor is not supervised. Its `append_message` call must carry the same
  // five arguments it carried before Task 6 — in particular NO `guard` key, so
  // the invoke Tauri sees is byte-identical to today's.
  const plan = persistAssistantTurn({ chatId: 4, content: 'Because the derivative is zero.', citations: null, calculations: [] });
  assert.strictEqual(plan.command, 'append_message');
  assert.deepStrictEqual(Object.keys(plan.args), ['chatId', 'role', 'content', 'citations', 'toolCalls']);
  assert.deepStrictEqual(plan.args, {
    chatId: 4, role: 'assistant', content: 'Because the derivative is zero.', citations: null, toolCalls: null,
  });
});

test('TUTOR IDENTITY: a row id from the mobile engine changes nothing without a verdict', () => {
  // `ChatEvent::Done` now carries the finalized row id for EVERY mobile turn,
  // the tutor's included. Only a guarded turn attaches; the tutor keeps its
  // existing append, duplicate row and all (fixing that is a different change).
  const plan = persistAssistantTurn({ chatId: 4, content: 'hi', messageId: 77, calculations: [] });
  assert.strictEqual(plan.command, 'append_message');
  assert.strictEqual('guard' in plan.args, false);
  assert.strictEqual('messageId' in plan.args, false);
});

test('TUTOR IDENTITY: no provisional banner is ever computed for an unsupervised turn', () => {
  assert.deepStrictEqual(
    provisionalStep({ supervised: false, alreadyShown: false, prefixText: 'Call an ambulance now.', elapsedMs: 10 }),
    { route: null, banner: null, log: null },
  );
});

test('TUTOR IDENTITY: the tutor is still titled by the model', () => {
  assert.deepStrictEqual(titlePlan({ supervised: false, source: 'explain photosynthesis' }), { kind: 'model' });
});
