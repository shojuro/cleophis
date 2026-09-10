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
import { assembleMessages } from './prompt-assembly.js';
import { windowMessages } from './context-window.js';
import { createTransport } from './transport.js';
import {
  ATTACH_FAILED_NOTICE, FOREIGN_CHAT_NOTICE, UNVERIFIED_BANNER, UNVERIFIED_TEXT,
  bannerKey, bannerText, canSendInChat, entryForChat, guardForPersistence, persistAssistantTurn,
  persistFailurePlan, provisionalStep, replayMessage, samplingFor, shouldGroundTurn,
  supervisedTitle, titlePlan,
} from './triage-turn.js';
import { applyGuard } from './triage/guard.js';

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

/* ---------------- the provenance stamped at persistence ---------------- */

const TRIAGE_ENTRY = Object.freeze({
  id: 'med-triage',
  supervised: true,
  promptFingerprint: '67b7f1633f30',
  sha256: '25162bffd5a8cf20079f78e6cac079f7b4f8fdd31403dd1a38177f2af450bfa3',
  adapterSha256: '5304e464cd485e8a7d8eb75083363e3cc4de0f665e2c785dbd1a1f7e93d13a20',
  sampling: { temperature: 0.0, maxTokens: 320 },
});

test('the PERSISTED verdict carries the three provenance keys, copied from the entry', () => {
  const verdict = applyGuard({ userText: 'chest pain', replyText: 'Call 999 now.' });
  const stamped = guardForPersistence(verdict, TRIAGE_ENTRY);
  assert.strictEqual(stamped.promptFingerprint, TRIAGE_ENTRY.promptFingerprint);
  assert.strictEqual(stamped.modelSha, TRIAGE_ENTRY.sha256);
  assert.strictEqual(stamped.adapterSha, TRIAGE_ENTRY.adapterSha256);
});

test('applyGuard\'s twelve-key verdict shape is NOT changed by the stamp', () => {
  // Task 3 pins the verdict's shape. Provenance is a fact about the turn, not
  // about the text, so it is added on a COPY at the moment the row is written.
  const verdict = applyGuard({ userText: 'chest pain', replyText: 'Call 999 now.' });
  const before = Object.keys(verdict).sort();
  const stamped = guardForPersistence(verdict, TRIAGE_ENTRY);
  assert.strictEqual(before.length, 12);
  assert.deepStrictEqual(Object.keys(verdict).sort(), before, 'applyGuard\'s verdict was mutated');
  assert.notStrictEqual(stamped, verdict);
  assert.deepStrictEqual(
    Object.keys(stamped).sort(),
    [...before, 'adapterSha', 'modelSha', 'promptFingerprint'].sort(),
  );
});

test('an entry that pins no shas stamps empty strings, never null and never a missing key', () => {
  // The export writes these as strings; one shape for every guarded row means
  // a reader never has to handle both `null` and `""` for "not recorded".
  const stamped = guardForPersistence({ route: 'SELF_CARE' }, { supervised: true });
  assert.deepStrictEqual(stamped, {
    route: 'SELF_CARE', promptFingerprint: '', modelSha: '', adapterSha: '',
  });
  const noEntry = guardForPersistence({ route: 'SELF_CARE' }, null);
  assert.deepStrictEqual(noEntry, {
    route: 'SELF_CARE', promptFingerprint: '', modelSha: '', adapterSha: '',
  });
});

test('TUTOR IDENTITY: no verdict means no stamp, so the invoke args are unchanged', () => {
  assert.strictEqual(guardForPersistence(null, TRIAGE_ENTRY), null);
  const plan = persistAssistantTurn({
    chatId: 4, content: 'Because the derivative is zero.',
    guard: guardForPersistence(null, { supervised: false }), messageId: 77,
  });
  assert.strictEqual(plan.command, 'append_message');
  assert.deepStrictEqual(Object.keys(plan.args).sort(), ['chatId', 'citations', 'content', 'role', 'toolCalls']);
});

/* ---------------- the sampling the desktop sidecar is asked for ---------------- */

test('a supervised entry\'s catalog sampling reaches the desktop request body', () => {
  assert.deepStrictEqual(
    samplingFor({ entry: TRIAGE_ENTRY, maxTokens: 1024, temperature: 0.7 }),
    { maxTokens: 320, temperature: 0.0 },
  );
});

test('temperature 0.0 is not treated as absent — the falsy trap this exists for', () => {
  // `s.temperature || fallback` would send 0.7 for exactly the entry whose
  // whole contract is that it is sampled at zero.
  const { temperature } = samplingFor({ entry: TRIAGE_ENTRY, maxTokens: 1024, temperature: 0.7 });
  assert.strictEqual(temperature, 0);
});

test('a supervised entry that pins only one value keeps the default for the other', () => {
  assert.deepStrictEqual(
    samplingFor({ entry: { supervised: true, sampling: { maxTokens: 64 } }, maxTokens: 1024, temperature: 0.7 }),
    { maxTokens: 64, temperature: 0.7 },
  );
  assert.deepStrictEqual(
    samplingFor({ entry: { supervised: true, sampling: {} }, maxTokens: 1024, temperature: 0.7 }),
    { maxTokens: 1024, temperature: 0.7 },
  );
});

test('TUTOR IDENTITY: an unsupervised entry gets the defaults it always got', () => {
  const defaults = { maxTokens: 1024, temperature: 0.7 };
  for (const entry of [
    null,
    { supervised: false },
    { supervised: null },
    {},
    // Not `=== true`, so it does not qualify — and a tutor entry that somehow
    // carried a `sampling` block still must not change what it sends.
    { supervised: 1, sampling: { temperature: 0.0, maxTokens: 8 } },
    { sampling: { temperature: 0.0, maxTokens: 8 } },
  ]) {
    assert.deepStrictEqual(samplingFor({ entry, ...defaults }), defaults, JSON.stringify(entry));
  }
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

/* ---------------- a persist that fails is never silent (round 1, I1) ------- */

test('a failed attach_guard is retried once, then surfaced — never swallowed', () => {
  // The row `settle` finalized holds the RAW reply. If `attach_guard` fails and
  // nothing says so, that raw reply is what is persisted, what a reopened chat
  // shows, and what the export omits: the guard silently fails toward showing
  // MORE than it decided to show.
  const first = persistFailurePlan({ command: 'attach_guard', attempt: 1, error: 'database is locked' });
  assert.strictEqual(first.action, 'retry');
  assert.strictEqual(first.log, '[triage] attach_guard failed database is locked');

  const second = persistFailurePlan({ command: 'attach_guard', attempt: 2, error: 'database is locked' });
  assert.strictEqual(second.action, 'surface');
  assert.strictEqual(second.message, ATTACH_FAILED_NOTICE);
  assert.strictEqual(second.message, 'This reply could not be verified and was not recorded — ask again');
  assert.strictEqual(second.log, '[triage] attach_guard failed database is locked');
});

test('the error is read off an Error object as well as a bare string', () => {
  const e = persistFailurePlan({ command: 'attach_guard', attempt: 2, error: new Error('no message with id 4') });
  assert.strictEqual(e.log, '[triage] attach_guard failed no message with id 4');
});

test('TUTOR IDENTITY: a failed append_message stays silent, exactly as it always was', () => {
  // The degrade-gracefully contract: a chat whose `create_chat` or
  // `append_message` failed keeps working locally and says nothing. That is the
  // tutor's behaviour today and this task does not change it — and it is not
  // the same failure, because a failed append leaves no raw reply behind.
  for (const attempt of [1, 2, 3]) {
    const plan = persistFailurePlan({ command: 'append_message', attempt, error: 'nope' });
    assert.deepStrictEqual(plan, { action: 'ignore', log: null, message: null });
  }
});

/* ---------------- an unguarded reply in a supervised chat (round 1, I2) ---- */

test('a supervised assistant row with NO verdict is withheld behind an Unverified reply banner', () => {
  // A partial row from a killed turn, a row whose `attach_guard` never landed,
  // any other producer: it reaches `rebuildChatDom` as an ordinary bubble with
  // the model's own words and no banner at all, which is the one thing a
  // supervised reply may never be. Fail toward showing less.
  const view = replayMessage({ supervised: true, role: 'assistant', content: 'Take 400mg of ibuprofen.', guard: null });
  assert.deepStrictEqual(view, {
    banner: 'unverified',
    text: 'This reply was not verified by the safety check and is not shown.',
    withheld: true,
  });
  assert.strictEqual(view.text, UNVERIFIED_TEXT);
  assert.strictEqual(view.banner, UNVERIFIED_BANNER);
});

test('the Unverified banner has its own copy and its own colour, and is not one of the four routes', () => {
  assert.strictEqual(bannerKey('unverified'), 'unverified');
  assert.strictEqual(bannerText('unverified').title, 'Unverified reply');
  for (const route of ['emergency', 'clinician', 'self_care', 'out_of_scope']) {
    assert.notStrictEqual(bannerText('unverified').title, bannerText(route).title);
  }
});

test('a supervised reply WITH a verdict renders its own banner and its own display text', () => {
  const guard = { banner: 'clinician', route: 'CLINICIAN', displayText: 'See a clinician.' };
  assert.deepStrictEqual(
    replayMessage({ supervised: true, role: 'assistant', content: 'See a clinician.', guard }),
    { banner: 'clinician', text: 'See a clinician.', withheld: false },
  );
});

test('a user turn in a supervised chat is never withheld and never bannered', () => {
  assert.deepStrictEqual(
    replayMessage({ supervised: true, role: 'user', content: 'he is short of breath', guard: null }),
    { banner: null, text: 'he is short of breath', withheld: false },
  );
});

test('TUTOR IDENTITY: an unguarded reply in an unsupervised chat renders its own content, unbannered', () => {
  assert.deepStrictEqual(
    replayMessage({ supervised: false, role: 'assistant', content: 'Because the derivative is zero.', guard: null }),
    { banner: null, text: 'Because the derivative is zero.', withheld: false },
  );
});

/* ---------------- whose chat is this? (round 2) --------------------------- */

const TUTOR = Object.freeze({ id: 'socratic-tutor', systemPrompt: 'tutor', greeting: 'Hi!' });
const TRIAGE = Object.freeze({ id: 'med-triage', supervised: true, systemPrompt: 'triage', greeting: 'Describe...' });
const CATALOG = Object.freeze([TUTOR, TRIAGE]);

test('a chat is rendered by the entry it BELONGS to, not by the one that happens to be entered', () => {
  assert.strictEqual(entryForChat(CATALOG, { modelId: 'socratic-tutor' }, TRIAGE), TUTOR);
  assert.strictEqual(entryForChat(CATALOG, { modelId: 'med-triage' }, TUTOR), TRIAGE);
});

test('a chat whose model is not in this catalog falls back to the entered model', () => {
  assert.strictEqual(entryForChat(CATALOG, { modelId: 'a-model-that-was-removed' }, TUTOR), TUTOR);
  assert.strictEqual(entryForChat(CATALOG, { modelId: '' }, TRIAGE), TRIAGE);
  assert.strictEqual(entryForChat(CATALOG, null, TRIAGE), TRIAGE);
  assert.strictEqual(entryForChat([], { modelId: 'x' }, null), null);
});

test('THE ROUND-1 REGRESSION: triage entered, a TUTOR chat opened — nothing is withheld', () => {
  // Round 1 asked "does this transcript hold a verdict, or is a supervised
  // model entered?", and the second term is true here for a chat that has
  // nothing to do with the triage assistant. Every tutor reply was withheld.
  const entry = entryForChat(CATALOG, { modelId: 'socratic-tutor' }, TRIAGE);
  const supervised = entry.supervised === true;
  assert.strictEqual(supervised, false);
  for (const content of ['q', 'Because the derivative is zero.']) {
    assert.deepStrictEqual(
      replayMessage({ supervised, role: 'assistant', content, guard: null }),
      { banner: null, text: content, withheld: false },
    );
  }
});

test('tutor entered, a TRIAGE chat opened — unguarded rows are withheld, guarded rows keep their banner', () => {
  const entry = entryForChat(CATALOG, { modelId: 'med-triage' }, TUTOR);
  const supervised = entry.supervised === true;
  assert.strictEqual(supervised, true);
  assert.deepStrictEqual(
    replayMessage({ supervised, role: 'assistant', content: 'Take 400mg of ibuprofen.', guard: null }),
    { banner: 'unverified', text: UNVERIFIED_TEXT, withheld: true },
  );
  assert.deepStrictEqual(
    replayMessage({ supervised, role: 'assistant', content: 'Go now.', guard: { banner: 'emergency' } }),
    { banner: 'emergency', text: 'Go now.', withheld: false },
  );
  assert.deepStrictEqual(
    replayMessage({ supervised, role: 'user', content: 'chest pain', guard: null }),
    { banner: null, text: 'chest pain', withheld: false },
  );
});

/* ---------------- a supervised assistant answers only its own chats ------- */

test('a supervised model refuses to answer in a chat that belongs to another assistant', () => {
  // The guard follows the ENTERED model, because that is the model that
  // answers. So sending in a foreign chat would write a guarded triage row into
  // a tutor conversation. The refusal is the fix; retitling the chat is not.
  const no = canSendInChat({ entered: TRIAGE, chat: { modelId: 'socratic-tutor' } });
  assert.strictEqual(no.allowed, false);
  assert.strictEqual(no.message, FOREIGN_CHAT_NOTICE);
  assert.strictEqual(no.message, 'This chat belongs to a different assistant — start a new chat for the triage assistant.');
});

test('a supervised model answers in its own chat, and in one that does not exist yet', () => {
  assert.deepStrictEqual(canSendInChat({ entered: TRIAGE, chat: { modelId: 'med-triage' } }), { allowed: true, message: null });
  // No chat record yet: the first message creates one, with this model's id.
  assert.deepStrictEqual(canSendInChat({ entered: TRIAGE, chat: null }), { allowed: true, message: null });
  assert.deepStrictEqual(canSendInChat({ entered: TRIAGE, chat: {} }), { allowed: true, message: null });
});

test('a chat that names no model at all is refused, not assumed', () => {
  // An empty `model_id` is a chat this cannot attribute. Refusing costs a new
  // chat; assuming writes a triage row somewhere nobody can account for.
  assert.strictEqual(canSendInChat({ entered: TRIAGE, chat: { modelId: '' } }).allowed, false);
});

test('TUTOR IDENTITY: an unsupervised model sends in any chat, exactly as it always did', () => {
  for (const chat of [null, {}, { modelId: '' }, { modelId: 'med-triage' }, { modelId: 'socratic-tutor' }]) {
    assert.deepStrictEqual(canSendInChat({ entered: TUTOR, chat }), { allowed: true, message: null });
  }
  assert.deepStrictEqual(canSendInChat({ entered: null, chat: { modelId: 'x' } }), { allowed: true, message: null });
  assert.deepStrictEqual(canSendInChat({}), { allowed: true, message: null });
});

/* ---------------- the producer, closed (round 1, I2) ---------------------- */

test('a supervised turn never grounds, however many packs are attached', () => {
  // Task 5's rule: the supervised system content is the catalog prompt and
  // nothing else. A grounded turn replaces it with the RAG prompt and the
  // no-evidence path appends a scripted refusal that no guard ever sees — both
  // are unguarded producers, and both are simply off here.
  assert.strictEqual(shouldGroundTurn({ supervised: true, packCount: 3 }), false);
  assert.strictEqual(shouldGroundTurn({ supervised: true, packCount: 0 }), false);
});

test('TUTOR IDENTITY: an unsupervised turn grounds exactly when packs are attached', () => {
  assert.strictEqual(shouldGroundTurn({ supervised: false, packCount: 1 }), true);
  assert.strictEqual(shouldGroundTurn({ supervised: false, packCount: 0 }), false);
  assert.strictEqual(shouldGroundTurn({}), false);
});

/* -------- TUTOR IDENTITY: the wire payload, end to end (round 1, I3) ------ */

// `windowMessages` hands `assembleMessages` the SAME message objects the app
// keeps in `state.chat.messages`, and those objects now carry `id` — and, on a
// supervised turn, the whole `guard` blob including `rawReply`. A spread would
// put every one of those fields on the wire: a different `chat_stream` payload
// for the tutor than before this task, and a supervised turn re-sending each
// earlier raw reply to the model. This is the test that says it does not.
function wirePayload({ entry, messages }) {
  const invoke = (() => {
    const calls = [];
    const f = async (cmd, args) => { calls.push({ cmd, args }); if (cmd === 'chat_stream') args.onEvent.onmessage?.({ event: 'done', data: { content: '', calculations: [] } }); };
    f.calls = calls;
    return f;
  })();
  const t = createTransport({
    mobile: true,
    invoke,
    Channel: class { set onmessage(f) { this._f = f; } get onmessage() { return this._f; } },
    newRequestId: () => 'req-1',
  });
  const { system: sys } = assembleMessages({ entry, groundedPrompt: null, sent: [], ungroundedNote: ' NOTE' });
  const win = windowMessages(messages, sys, entry.greeting, 4096);
  const assembled = assembleMessages({ entry, groundedPrompt: null, sent: win.sent, ungroundedNote: ' NOTE' });
  t.streamTurn({ chatId: 1, messages: assembled.messages, onDelta: () => {} });
  return invoke.calls.find((c) => c.cmd === 'chat_stream').args.messages;
}

test('TUTOR IDENTITY: chat_stream carries {role, content} and nothing else', () => {
  const entry = { supervised: false, systemPrompt: 'You are a tutor.', greeting: 'Hi!' };
  const sent = wirePayload({
    entry,
    messages: [
      { role: 'user', content: 'q1' },
      { role: 'assistant', content: 'a1', id: 42, citations: [{ docTitle: 'd' }], calculations: [{ expression: '1+1', display: '2' }] },
      { role: 'user', content: 'q2' },
    ],
  });
  assert.deepStrictEqual(sent, [
    { role: 'system', content: 'You are a tutor. NOTE' },
    { role: 'assistant', content: 'Hi!' },
    { role: 'user', content: 'q1' },
    { role: 'assistant', content: 'a1' },
    { role: 'user', content: 'q2' },
  ]);
  for (const m of sent) assert.deepStrictEqual(Object.keys(m), ['role', 'content']);
});

test('a supervised turn never re-sends an earlier reply\'s verdict to the model', () => {
  const entry = { supervised: true, systemPrompt: 'You are a triage assistant.', greeting: 'Describe…' };
  const sent = wirePayload({
    entry,
    messages: [
      { role: 'user', content: 'chest pain' },
      { role: 'assistant', content: 'Go now.', id: 7, guard: { route: 'EMERGENCY', rawReply: 'Go now, within 10 minutes.', detectorsSha: 'abc' } },
      { role: 'user', content: 'and now?' },
    ],
  });
  assert.deepStrictEqual(sent, [
    { role: 'system', content: 'You are a triage assistant.' },
    { role: 'user', content: 'chest pain' },
    { role: 'assistant', content: 'Go now.' },
    { role: 'user', content: 'and now?' },
  ]);
  assert.strictEqual(JSON.stringify(sent).includes('rawReply'), false);
  assert.strictEqual(JSON.stringify(sent).includes('within 10 minutes'), false);
});
