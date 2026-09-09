// Cleophis front-end. Requires app.withGlobalTauri=true.
import { CALC_TOOL } from './calc-tool.js';
import { streamWithTools } from './calc-loop.js';
import { createTransport, isAndroid } from './transport.js';
import { describeEngineState, createReadableSequence, PREFILL_EXPLAIN_MS, THERMAL_PROMINENT_MS } from './engine-state.js';
import { windowMessages, engineWindow, REPLY_RESERVE } from './context-window.js';
import { decideDownload, meteredPromptText } from './download-policy.js';
import { assembleMessages } from './prompt-assembly.js';
import { belowMinTier, minTierNotice, tierSelectorApplies } from './min-tier.js';
// The product's contract on a supervised reply (Phase 2). Every use below is
// gated on `entry.supervised === true`; the tutor never reaches any of it.
import { applyGuard } from './triage/guard.js';
import {
  bannerKey, bannerText, chatIsSupervised, persistAssistantTurn, persistFailurePlan,
  provisionalStep, replayMessage, shouldGroundTurn, titlePlan,
} from './triage-turn.js';
import { isPromptMismatch, promptFingerprint } from './prompt-fingerprint.js';

const { invoke, convertFileSrc, Channel } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

// The chat transport, chosen ONCE here (task 2.1). Desktop talks to the
// `llama-server` sidecar over loopback and runs the tool loop in JS; Android
// has no server to talk to and runs it in-process behind `invoke`. Every
// engine call in this file goes through this object — there are exactly two,
// and grepping for `127.0.0.1` should keep finding nothing outside
// transport.js.
const transport = createTransport({
  mobile: isAndroid(navigator.userAgent),
  invoke,
  Channel,
  streamImpl: streamWithTools,
});

// Task 2.2: one platform predicate, shared with the transport above, decides
// BOTH which half of the seam is live and which stylesheet rules apply. Every
// mobile CSS rule is scoped under `.is-mobile` rather than behind a media
// query, so a desktop build matches none of them at any window width and
// desktop behaviour stays byte-identical — the same reasoning that put the
// transport choice in one place instead of at each call site.
const IS_MOBILE = isAndroid(navigator.userAgent);
document.documentElement.classList.toggle('is-mobile', IS_MOBILE);

// Keep the layout on the VISUAL viewport so the soft keyboard cannot cover the
// composer. `100vh` is the initial viewport and never shrinks for the IME; the
// manifest's `adjustResize` is the other half of this, but an edge-to-edge
// activity on newer Android may ignore it, and `visualViewport` reports the
// truth either way. Desktop never runs this.
if (IS_MOBILE && window.visualViewport) {
  const syncViewportHeight = () => {
    document.documentElement.style.setProperty('--app-h', `${window.visualViewport.height}px`);
  };
  window.visualViewport.addEventListener('resize', syncViewportHeight);
  syncViewportHeight();
}

// Chat lock-out threshold for lapsed subscriptions: local expiry dates can
// be stale (offline-first — Stripe may have renewed while this machine was
// offline), so chat keeps working for a grace window past the known expiry
// and only then routes to Renew. Downloads are unaffected: the server
// gates those at exact expiry.
const LAPSE_GRACE_MS = 7 * 24 * 3600 * 1000;

const state = {
  cat: 'all', subject: 'all', q: '', signedIn: false, nick: null, device: null,
  mine: new Set(), lapsed: new Set(), chatBlocked: new Set(), catalog: [],
  engine: { port: 0, status: 'Starting', gpuOffload: false },
  chat: { model: null, messages: [], streaming: false, aborter: null, packPaths: [], chatId: null },
  dl: { installed: false, partBytes: 0, active: false },
  // Task 2.2 engine-state inputs. `dlProgress` is the last download-progress
  // payload (null when no download is in flight), `turn` times the in-flight
  // turn so the prefill wait can be explained from a real signal rather than
  // guessed, and `hw` caches detect_hardware so `supported:false` (task 2.3)
  // can drive the honest "not yet" screen.
  // `thermal` is the last `thermal-notice` payload (task 5.1, hazard H6): the
  // backend emits on BOTH edges, so this is set by onset and cleared by
  // recovery rather than latching a warning nothing withdraws.
  dlProgress: null, turn: null, hw: null, thermal: null,
  // B4: while a hero dist-catalog download is awaiting a SPECIFIC artifact's
  // terminal event, this holds { path, resolve, reject } so onDownloadProgress
  // routes that artifact's done/failed/cancelled to the sequencing promise
  // (base then adapter) instead of the legacy single-file hero handling.
  artDl: null,
  pay: { modelId: null, timer: null, deadline: 0, btnId: null },
  drawerId: null,
  // §7 S7-2b: sidebar organization — folders/chats cache backing the
  // grouped render, plus the search view and the three single-open
  // dropdowns (a chat's "Move to…" list, a chat's export-format menu
  // [§7 S7-6], a folder's Rename/Delete menu).
  sidebar: { chats: [], folders: [], showArchived: false, query: '', searchResults: null, moveMenuFor: null, folderMenuFor: null, exportMenuFor: null, expandedFolders: new Set() },
  // Wrapper tier-selection state from get_tier_selection: { mode, activeTier,
  // effectiveTier, committed, switchAvailable, nextChangeAt }.
  tierSel: null,
  // Re-entrancy guard for an in-flight tier switch (rapid clicks must not
  // overlap complete_tier_switch → engine restart), and a once-per-session
  // latch so the first chat fires a single mark_tier_committed round-trip.
  tierSwitching: false, tierCommitted: false,
};

const $ = (id) => document.getElementById(id);

// Defense-in-depth: catalog fields are interpolated into innerHTML templates
// below. The catalog is a bundled local file today (not remotely fetched),
// but escape it anyway in case that changes or the resource file is
// tampered with.
function escapeHtml(s) {
  return String(s ?? '').replace(/[&<>"']/g, (c) => (
    { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]
  ));
}

// B4: the Qwen3 hero opens each turn with an EMPTY reasoning block
// (`<think></think>`) before its visible answer. The multi-round tool loop
// (one generation per round under `--jinja`) means this quirk can fire on
// MORE THAN ONE round of the same turn (e.g. the tool-call round AND the
// answer round), which accumulates as `<think></think><think></think>...`
// once the rounds' content is concatenated. Strip ONE-OR-MORE consecutive
// leading empty-think blocks — optionally whitespace-wrapped — and ONLY at
// the very start of the turn. The pattern is anchored at `^` and requires
// each block to be empty (only whitespace between the tags), so it:
//   • never touches a NON-empty <think>…</think> (real reasoning is left in
//     place — it just won't occur for this always-on behavioral adapter),
//   • never strips anything mid-answer (a `</think>` appearing after real
//     text is not at `^`, so it can't match), and
//   • leaves a turn that doesn't start with the pattern 100% untouched.
// `String.replace` with this anchored regex removes the entire leading run
// in one pass, and re-running it on a longer `acc` is idempotent (the same
// leading prefix is removed each time), so it is safe to call on every
// partial render as well as on the final persisted text.
function stripLeadingThink(text) {
  return String(text).replace(/^(?:\s*<think>\s*<\/think>\s*)+/, '');
}

// Wrapper tier-selection: the hero installs/runs the base+adapter for the
// EFFECTIVE device tier (override, else detected). The bundled catalog entry
// carries a per-tier `tiers` block; `get_tier_selection` (Rust) resolves the
// effective tier + the switch-limit state, cached in `state.tierSel`. These
// helpers keep the download picker, the selector UI, and the provenance stamp
// all reading the same source, so the FE selection can't drift.
function heroEntry() { return state.catalog.find((m) => m.real); }
function effectiveTier() { return (state.tierSel && state.tierSel.effectiveTier) || 'mid'; }
function tierVariant(tier) {
  const h = heroEntry();
  return h && h.tiers && h.tiers[tier] ? h.tiers[tier] : null;
}
// The dist-catalog `base_model` for a tier (the kind:'base'/'adapter' records
// carry it). Falls back to the 4B hero if the catalog predates `tiers`.
function heroBaseModel(tier) {
  const v = tierVariant(tier || effectiveTier());
  return v && v.baseModel ? v.baseModel : 'Qwen3-4B';
}
// The always-on adapter id stamped as per-chat provenance — the ACTIVE tier's,
// not always the 4B one.
function heroAdapterId(m) {
  if (m && m.real) {
    const v = tierVariant(effectiveTier());
    if (v && v.adapterId) return v.adapterId;
  }
  return m && m.adapterId ? m.adapterId : null;
}

/* ---------------- boot ---------------- */
async function boot() {
  await listen('engine-ready', (e) => { state.engine = e.payload; hideEngineBanner(); setComposerEnabled(true); refreshEngineState(); });
  await listen('engine-restarting', () => { showEngineBanner('Local engine restarting…'); setComposerEnabled(false); state.engine = { ...state.engine, status: 'Restarting' }; refreshEngineState(); });
  await listen('engine-failed', async (e) => {
    setComposerEnabled(false);
    state.engine = { ...state.engine, status: 'Failed' };
    refreshEngineState();
    // A failed integrity check almost always means a required model file is
    // missing (e.g. an install predating a newly-required adapter, or one the
    // tier-switch sweep removed). When download_status agrees the hero isn't
    // fully installed, offer an in-place re-download — the "re-download it"
    // message is useless without a control, especially from inside a chat.
    // heroDownload fetches only the missing file(s), then load_model restarts
    // the engine off Failed and re-enters the chat (engine-ready clears this).
    const hero = state.catalog.find((m) => m.real);
    let notInstalled = false;
    if (hero) {
      try { notInstalled = !(await invoke('download_status', { modelId: hero.id })).installed; } catch (_) {}
    }
    if (hero && notInstalled) {
      showEngineBanner('Local engine failed — a required model file is missing.', {
        label: 'Re-download',
        onClick: () => heroDownload(hero, $('engineBanner').querySelector('.banner-action')),
      });
    } else {
      showEngineBanner('Local engine failed: ' + e.payload);
    }
  });
  // Thermal throttling (task 5.1, hazard H6, acceptance A7). `at` is stamped on
  // arrival rather than sent by the backend: the payload's job is the verdict,
  // and the only clock the prominence decay may compare against is the one
  // `refreshEngineState` reads.
  await listen('thermal-notice', (e) => {
    state.thermal = { ...e.payload, at: Date.now() };
    refreshEngineState();
    // The row demotes itself to the pill on a timer, and no other event is
    // guaranteed to arrive while the user simply reads the reply.
    setTimeout(refreshEngineState, THERMAL_PROMINENT_MS + 30);
  });
  await listen('download-progress', onDownloadProgress);
  await listen('build-progress', onBuildProgress);
  state.catalog = await invoke('get_catalog');
  for (const m of state.catalog) m.coverUrl = convertFileSrc(m.coverAbs);
  // Tier-selection state first — download_status is now tier-aware, so the
  // effective tier must be known before we ask whether the hero is installed.
  try { state.tierSel = await invoke('get_tier_selection'); } catch (_) {}
  const hero = state.catalog.find((m) => m.real);
  if (hero) {
    try {
      const ds = await invoke('download_status', { modelId: hero.id });
      state.dl = { installed: ds.installed, partBytes: ds.partBytes, active: ds.active };
    } catch (_) {}
  }
  try { state.engine = await invoke('engine_info'); } catch (_) {}
  // Re-assert the thermal verdict (H6/A7). `thermal-notice` fires only on
  // transitions and the watch outlives this webview, so if the renderer was
  // killed under memory pressure mid-session there is no future event to
  // recover from — the backend would sit on `notified = true` forever.
  //
  // A re-assert is deliberately NOT news: the user has already been told, and
  // re-raising the full-width row on every reload would make the loudest
  // element on screen a fact they read ten minutes ago. Backdating `at` by the
  // prominence window lands it straight in the pill, still visible and still
  // honest, using the decay `describeEngineState` already implements rather
  // than a second code path.
  if (IS_MOBILE) {
    try {
      const t = await invoke('chat_thermal_state');
      if (t && t.throttled) state.thermal = { ...t, at: Date.now() - THERMAL_PROMINENT_MS };
    } catch (_) {}
    // Q1: reveal the throttle-notice selftest, but only where it exists. The
    // probe runs the command with `run: false`, which changes nothing and emits
    // nothing — a release build's body is compiled out and refuses, so the
    // button never appears there. Feature-detected rather than inferred from a
    // build flag the frontend would have to be told about separately, which is
    // the two-homes-for-one-fact trap D-4 is about.
    try {
      await invoke('chat_thermal_selftest', { run: false });
      document.documentElement.classList.add('is-debug');
    } catch (_) {}
  }
  // Re-apply the stored FLAG_SECURE preference (§5.3). Window flags do not
  // survive a process restart, so a stored `on` that is never re-applied is the
  // worst of both worlds: the toggle reads On and the window is not secure.
  await applyScreenPrivacy(screenPrivacyOn());
  // Task 2.2/2.3: ask the device what it can do BEFORE the onboarding funnel,
  // not at sign-in like desktop does. A phone that cannot run a model should
  // be told so before it is walked through account → payment.
  if (IS_MOBILE) {
    try { state.hw = await invoke('detect_hardware'); } catch (_) {}
    if (state.hw && state.hw.supported === false) showNotYetScreen(state.hw);
  }
  refreshEngineState();
  renderFilters(); renderGrid();
  try {
    const s = await invoke('restore_session');
    if (s.signedIn) await applySession(s);
  } catch (_) { /* signed-out boot is fine */ }
}

/* ---------------- screen privacy (FLAG_SECURE, spec §5.3) ---------------- */

// Stored on the DEVICE, not the account. This describes one phone — the one you
// hand to other people — so syncing it would turn a per-device precaution into a
// global setting and quietly enable it on a laptop nobody else touches.
const SCREEN_PRIVACY_KEY = 'cleophis.screenPrivacy';

/// Default OFF (§5.3: screenshots are the user's right). A missing key, an
/// unreadable store, and an explicit "off" all mean the same thing, so they all
/// return false rather than being distinguished into a third state.
function screenPrivacyOn() {
  try { return localStorage.getItem(SCREEN_PRIVACY_KEY) === '1'; } catch (_) { return false; }
}

/// Push the preference to the platform and reflect it in the menu.
///
/// The label carries the state rather than a checkmark glyph, because this menu
/// is a list of actions and a toggle that looks identical to "Sign out" would be
/// pressed by accident. `aria-pressed` is what actually tells a screen reader it
/// is a toggle.
async function applyScreenPrivacy(on) {
  const btn = $('screenPrivacyBtn');
  if (btn) {
    btn.textContent = 'Hide in app switcher · ' + (on ? 'On' : 'Off');
    btn.setAttribute('aria-pressed', on ? 'true' : 'false');
  }
  if (!IS_MOBILE) return;
  // A failure here is silent on purpose but NOT ignored: the catch keeps a
  // bridge error from breaking boot, and the eprintln on the Rust side is what
  // a device log shows. There is deliberately no "privacy enabled" toast — a
  // confirmation the app cannot actually verify would be worse than none.
  try { await invoke('set_screen_privacy', { secure: on }); } catch (_) {}
}

/* ---------------- compatibility ---------------- */
function compat(sizeParams) {
  if (!state.signedIn || !state.device) return { cls: 'unknown', label: 'Sign in to check' };
  const b = parseInt(sizeParams, 10);
  const t = state.device.tier;
  if (t === 'high') return b <= 7 ? c('great', 'Runs great') : c('well', 'Runs well');
  if (t === 'mid') { if (b <= 3) return c('great', 'Runs great'); if (b <= 7) return c('well', 'Runs well'); return c('heavy', 'Heavy'); }
  if (b <= 1) return c('great', 'Runs great');
  if (b <= 3) return c('well', 'Runs well');
  return c('heavy', 'Heavy');
  function c(cls, label) { return { cls, label }; }
}

/* ---------------- catalog rendering ---------------- */
function subjectsFor(cat) {
  const set = new Set(state.catalog
    .filter((m) => cat === 'all' || cat === 'mine' || m.category === cat)
    .map((m) => m.subject));
  return ['all', ...set];
}

function renderFilters() {
  $('subjectFilters').innerHTML = subjectsFor(state.cat).map((s) =>
    `<button class="chip ${state.subject === s ? 'active' : ''}" data-subject="${escapeHtml(s)}">${s === 'all' ? 'All subjects' : escapeHtml(s)}</button>`
  ).join('') + `<span class="count" id="count"></span>`;
}

function visible() {
  return state.catalog.filter((m) => {
    if (state.cat === 'mine') { if (!state.mine.has(m.id)) return false; }
    else if (state.cat !== 'all' && m.category !== state.cat) return false;
    if (state.subject !== 'all' && m.subject !== state.subject) return false;
    if (state.q) {
      const q = state.q.toLowerCase();
      if (!(m.name + m.subject + m.blurb).toLowerCase().includes(q)) return false;
    }
    return true;
  });
}

function renderGrid() {
  const list = visible();
  const cnt = $('count'); if (cnt) cnt.textContent = `${list.length} model${list.length === 1 ? '' : 's'}`;
  if (!list.length) {
    $('grid').innerHTML = `<div style="color:var(--muted);padding:30px 4px;grid-column:1/-1">
      ${state.cat === 'mine' ? 'Your library is empty. Open any model and choose Get to add it here.' : 'No models match — try a different subject or search.'}</div>`;
    return;
  }
  $('grid').innerHTML = list.map((m) => {
    const cp = compat(m.sizeParams);
    return `<button class="card" data-id="${escapeHtml(m.id)}">
      <div class="cover"><img src="${escapeHtml(m.coverUrl)}" alt="" loading="lazy"/>
        <span class="tag ${escapeHtml(m.category)}">${m.category === 'education' ? 'Education' : 'Medical'}</span></div>
      <div class="cardbody">
        <h3>${escapeHtml(m.name)}</h3>
        <p class="sub">${escapeHtml(m.subject)}</p>
        <div class="cardfoot">
          <span class="compat ${cp.cls}">${cp.label}</span>
          <span class="meta mono">${escapeHtml(m.sizeParams)} · ${m.pro ? 'Pro' : escapeHtml(m.price)}</span>
        </div>
      </div>
    </button>`;
  }).join('');
}

/* ---------------- tier selector ---------------- */
// "Pick your engine size" — only shown for the hero, once owned/installed.
// Reads state.tierSel (from get_tier_selection). The active mode is
// highlighted; when a change isn't available this period, the other options
// are disabled with a hint.
const TIER_OPTS = [
  { mode: 'auto', label: 'Auto', sub: 'match my device' },
  { mode: 'low', label: 'Small · 1B', sub: '≈0.8 GB · fastest' },
  { mode: 'mid', label: 'Balanced · 4B', sub: '≈2.4 GB' },
  { mode: 'high', label: 'Large · 8B', sub: '≈4.8 GB · most capable' },
];
function tierSelectorHtml(m) {
  // P2.9: also renders nothing for an entry with no `tiers` block. The options
  // below ARE that block; without it the selector would offer three models the
  // entry does not have.
  if (!tierSelectorApplies(m, { owned: state.mine.has(m.id), installed: state.dl.installed })) return '';
  const ts = state.tierSel || { mode: 'auto', effectiveTier: 'mid', switchAvailable: true, nextChangeAt: null };
  const opts = TIER_OPTS.map((o) => {
    const active = ts.mode === o.mode;
    const sub = o.mode === 'auto' ? `detected: ${escapeHtml(ts.effectiveTier)}` : o.sub;
    const disabled = !ts.switchAvailable && !active;
    return `<button class="tieropt${active ? ' active' : ''}" data-tier="${o.mode}"${disabled ? ' disabled' : ''}>
      <span class="tieropt-l">${o.label}</span><span class="tieropt-s mono">${sub}</span></button>`;
  }).join('');
  let note = '';
  if (!ts.switchAvailable) {
    note = ts.nextChangeAt
      ? `One change per billing period — next change after ${escapeHtml(new Date(ts.nextChangeAt).toLocaleDateString())}.`
      : 'You can change your model once per billing period.';
  }
  return `<div class="tiersel">
    <div class="tiersel-h">Pick your engine size</div>
    <div class="tiersel-opts">${opts}</div>
    ${note ? `<div class="tiersel-note mono">${note}</div>` : ''}
  </div>`;
}

/* ---------------- drawer ---------------- */
function openDrawer(id) {
  const m = state.catalog.find((x) => x.id === id); if (!m) return;
  state.drawerId = m.id;
  const cp = compat(m.sizeParams);
  const installed = state.mine.has(m.id);
  const lapsed = m.real && state.lapsed.has(m.id);
  const chatBlocked = m.real && state.chatBlocked.has(m.id);
  // For the hero, show the EFFECTIVE tier's size (a low/high device isn't
  // downloading the 4B), not the flat/mid fileBytes.
  const heroBytes = (m.real && tierVariant(effectiveTier())) ? tierVariant(effectiveTier()).fileBytes : m.fileBytes;
  const gb = (heroBytes / 2 ** 30).toFixed(2);
  const check = '<svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><path d="M20 6L9 17l-5-5"/></svg>';
  const gib = (heroBytes / 2 ** 30).toFixed(2);
  const waitingLabel = 'Waiting for payment… (click to cancel)';
  // P2.9: an entry may declare the lowest device tier it runs acceptably on.
  // Below it the Get / Download control is REPLACED by a line saying so —
  // replaced rather than disabled, because a dead button invites a second tap
  // and explains nothing. Entries without a `minTier` (every entry today but
  // the triage one) are untouched.
  const belowMin = belowMinTier(effectiveTier(), m.minTier);
  const btnLabel = m.real
    ? (!installed
        ? (state.pay.modelId === m.id
            ? waitingLabel
            : `Get · ${m.pro ? 'Pro' : escapeHtml(m.price)}`)
        : ((lapsed && !state.dl.installed) || chatBlocked
            ? (state.pay.modelId === m.id
                ? waitingLabel
                : `Renew · ${m.pro ? 'Pro' : escapeHtml(m.price)}`)
            : (state.dl.installed
                ? 'Open chat'
                : state.dl.active
                  ? 'Downloading…'
                  : state.dl.partBytes > 0
                    ? `Resume download · ${(state.dl.partBytes / 2 ** 30).toFixed(2)} of ${gib} GiB`
                    : `Download · ${gib} GiB`)))
    : (installed ? 'Installed' : `Download · ${m.pro ? 'Pro' : escapeHtml(m.price)}`);
  const showRenewLine = lapsed && state.dl.installed && !chatBlocked;
  const renewLineHtml = showRenewLine
    ? `<div class="dlline mono" id="renewLine" style="display:block;font-size:12.5px;color:var(--muted);margin-top:8px;cursor:pointer">${state.pay.modelId === m.id ? waitingLabel : 'Subscription lapsed — downloads paused. Renew · $20/mo'}</div>`
    : '';
  $('drawer').innerHTML = `
    <button class="x" data-close>&times;</button>
    <div class="dcover"><img src="${escapeHtml(m.coverUrl)}" alt=""/></div>
    <span class="tag ${escapeHtml(m.category)}" style="position:static;display:inline-block;margin-top:14px">${m.category === 'education' ? 'Education' : 'Medical reference'}</span>
    <h2>${escapeHtml(m.name)}</h2>
    <div class="dsub">${escapeHtml(m.subject)} · ${gb} GiB on disk</div>
    <div class="specs">
      <div class="spec"><div class="k">Model size</div><div class="v mono">${escapeHtml(m.sizeParams)} params</div></div>
      <div class="spec"><div class="k">On your device</div><div class="v"><span class="compat ${cp.cls}">${cp.label}</span></div></div>
      <div class="spec"><div class="k">Speed</div><div class="v mono">${escapeHtml(m.tps) || '—'}</div></div>
      <div class="spec"><div class="k">Eval</div><div class="v">${escapeHtml(m.eval) || '—'}</div></div>
    </div>
    <div class="dlrow">
      ${belowMin
        ? `<div class="dlline mono" id="minTierLine" style="display:block;font-size:12.5px;color:var(--muted)">${escapeHtml(minTierNotice(m.minTier))}</div>`
        : `<button class="btn primary block" id="dlBtn">${btnLabel}</button>`}
      <div class="prog" id="prog"><i></i></div>
      <div class="dlline mono" id="dlLine" style="display:none;font-size:12.5px;color:var(--muted);margin-top:8px"></div>
      <div class="installed" id="installedMsg">${check} Installed — runs offline on your device</div>
      <div class="errmsg" id="errMsg"></div>
      ${renewLineHtml}
    </div>
    ${tierSelectorHtml(m)}
    <div class="body">
      <h4>About</h4><p>${escapeHtml(m.long || m.blurb)}</p>
      <h4>What's inside</h4>
      <ul class="inside">${(m.inside || []).map((i) => `<li>${check}${escapeHtml(i)}</li>`).join('')}</ul>
    </div>`;
  $('scrim').classList.add('show'); $('drawer').classList.add('show');
  $('drawer').setAttribute('aria-hidden', 'false');
  if (installed && !m.real) $('installedMsg').style.display = 'flex';
  // The button is absent entirely when the device is below the entry's floor.
  if (!belowMin) $('dlBtn').onclick = () => runGetFlow(m, $('dlBtn'));
  if (showRenewLine) {
    const renewEl = $('renewLine');
    renewEl.onclick = () => startCheckoutFlow(m, renewEl);
  }
  $('drawer').querySelectorAll('.tieropt[data-tier]').forEach((b) => {
    b.onclick = () => {
      if (b.disabled || b.classList.contains('active')) return;
      selectTier(m, b.dataset.tier);
    };
  });
}

function closeDrawer() {
  $('scrim').classList.remove('show'); $('drawer').classList.remove('show');
  $('drawer').setAttribute('aria-hidden', 'true');
  state.drawerId = null;
}

/* ---------------- Get flow ---------------- */
function runGetFlow(m, btn) {
  if (m.real) {
    const owned = state.mine.has(m.id);
    if (!owned) { startCheckoutFlow(m, btn); return; }
    if (state.chatBlocked.has(m.id)) { startCheckoutFlow(m, btn); return; }
    if (state.dl.installed) { enterChat(m); return; }
    if (state.lapsed.has(m.id)) { startCheckoutFlow(m, btn); return; }
    if (state.dl.active) return;
    renderGrid();
    heroDownload(m, btn);
  } else {
    if (state.mine.has(m.id)) return;
    simulateStubDownload(m, btn);
  }
}

/* ---------------- checkout flow ---------------- */
// The idle (non-pending) label for a model's primary checkout control:
// "Get" for never-owned models, "Renew" once the subscription has lapsed.
function primaryLabel(m) {
  return `${state.lapsed.has(m.id) ? 'Renew' : 'Get'} · ${m.pro ? 'Pro' : m.price}`;
}

// Like primaryLabel, but aware that the lapsed-and-installed drawer state
// drives its checkout from a separate renew line (not the primary dlBtn,
// which must keep reading "Open chat") — that element gets its own fixed
// idle copy instead of the generic Get/Renew short form.
function idleLabel(m, btn) {
  return (btn && btn.id === 'renewLine')
    ? 'Subscription lapsed — downloads paused. Renew · $20/mo'
    : primaryLabel(m);
}

// Shared by three callers with identical behavior: the not-owned Get button,
// the lapsed-not-installed primary (Renew) button, and the lapsed-installed
// renew line. `btn` is whichever element triggered the flow — its own
// disabled/text state is updated, never a different, unrelated control.
let checkoutOpening = false;
function startCheckoutFlow(m, btn) {
  if (state.pay.modelId === m.id) {
    cancelPaymentPoll(idleLabel(m, btn), btn);
    return;
  }
  // The renew control is a div — `disabled` is inert on it — so an explicit
  // in-flight flag covers the window before state.pay.modelId is set.
  if (checkoutOpening) return;
  checkoutOpening = true;
  btn.disabled = true;
  btn.textContent = 'Opening checkout…';
  (async () => {
    let res;
    try {
      res = await invoke('start_checkout', { modelId: m.id });
    } catch (e) {
      btn.disabled = false;
      btn.textContent = idleLabel(m, btn);
      const el = $('errMsg'); el.style.display = 'block'; el.style.color = ''; el.textContent = String(e);
      return;
    }
    if (res.status === 'alreadyOwned') {
      state.mine.add(m.id);
      state.lapsed.delete(m.id);
      state.chatBlocked.delete(m.id);
      renderGrid();
      if (state.dl.installed) { finishInstalled(m, btn); return; }
      btn.textContent = '✓ Owned — starting download…';
      heroDownload(m, btn);
      return;
    }
    btn.disabled = false;
    btn.textContent = 'Waiting for payment… (click to cancel)';
    const el = $('errMsg');
    el.style.display = 'block';
    el.style.color = 'var(--muted)';
    el.textContent = 'Complete your purchase in the browser window — this screen updates automatically.';
    beginPaymentPoll(m, btn.id);
  })().finally(() => { checkoutOpening = false; });
}

// B4: the hero now installs through the SIGNED DISTRIBUTION CATALOG, not the
// legacy minted `download_model` path. Fetch the verified dist catalog, pick
// the base + adapter artifacts for HERO_BASE_MODEL, then download them ONE AT
// A TIME (single-slot Downloads registry) via download_artifact. Only when
// BOTH files have landed is the hero considered installed and the engine
// loaded — a base-only state is never treated as installed (fail-closed,
// mirroring inference::resolve_launch + download_status). The
// entitlement/purchase gating that led here (runGetFlow / beginPaymentPoll /
// alreadyOwned) is unchanged — only the transport is new.
// Download the base+adapter pair for a given dist-catalog `baseModel`, ONE AT
// A TIME (single-slot Downloads registry). Resolves only when BOTH files have
// landed on disk; throws on a missing artifact or a download failure. Shared
// by the initial hero install and a tier switch (which pass different
// baseModels). The bar/line UI in onDownloadProgress is driven by the
// download-progress events download_artifact emits.
// The tiers-block variant for a given dist-catalog base_model (used to learn
// whether that tier declares a contract adapter).
function tierVariantByBase(baseModel) {
  const h = heroEntry();
  if (!h || !h.tiers) return null;
  return Object.values(h.tiers).find((t) => t.baseModel === baseModel) || null;
}

// §2.2 download network policy, Android only. The gate runs ONCE for the whole
// install rather than per artifact: base + adapter (+ contract) are one user
// action, and asking twice for one decision is how a prompt becomes noise.
//
// Desktop is untouched — `IS_MOBILE` is the same platform predicate that picks
// the transport (decision D-5), not a viewport check, so a narrow desktop
// window never sees this.
//
// The decision itself is `download-policy.js`, tested by `npm test`; the facts
// come from `network_state`, whose parse is tested in the desktop suite. If the
// command fails outright we do NOT block: the Rust side already falls back to
// "metered" internally, and a bridge failure that made the app un-downloadable
// would be a worse outcome than an unasked-for prompt.
async function meteredGate(totalBytes) {
  if (!IS_MOBILE) return true;
  let net;
  try {
    net = await invoke('network_state');
  } catch (e) {
    console.warn('network_state unavailable, proceeding:', e);
    return true;
  }
  const verdict = decideDownload(net, { bytes: totalBytes });
  if (verdict.chargeNotice) showToast(verdict.chargeNotice);
  if (verdict.allowed) return true;
  // The per-download override: this answer applies to this install and is
  // never remembered, so a user who says yes once on the train is not opted
  // in forever.
  return window.confirm(`${meteredPromptText(totalBytes)}\n\nDownload anyway?`);
}

async function downloadHeroPair(baseModel, btn) {
  const prog = $('prog');
  if (prog) prog.style.display = 'block';
  if (btn) { btn.disabled = true; btn.textContent = 'Downloading…'; }
  state.dl.active = true;
  try {
    const cat = await invoke('fetch_dist_catalog');
    const arts = (cat && cat.artifacts) || [];
    const base = arts.find((a) => a.kind === 'base' && a.base_model === baseModel);
    const adapter = arts.find((a) => a.kind === 'adapter' && a.base_model === baseModel);
    if (!base || !adapter) {
      throw new Error("This model isn't available to download yet — please update the app or try again later.");
    }
    // Totalled across everything this install will pull, including the
    // contract adapter when the tier declares one — the user is being asked
    // about their data allowance, and a figure that undercounts what will
    // actually be transferred is the wrong number to answer with.
    const variant = tierVariantByBase(baseModel);
    const contractArt = variant && variant.contractAdapterFile
      ? arts.find((a) => a.kind === 'contract-adapter' && a.base_model === baseModel)
      : null;
    const totalBytes = (base.size || 0) + (adapter.size || 0) + ((contractArt && contractArt.size) || 0);
    if (!(await meteredGate(totalBytes))) {
      // A declined prompt is a cancellation, not a failure: no toast, no error
      // chip. The `finally` below restores the button and the progress bar.
      return;
    }
    await downloadArtifact(base);
    await downloadArtifact(adapter);
    // Adapter v2 static composition: if this tier declares a contract adapter,
    // download it too (composed alongside the behavioral one via `--lora a,b`).
    // Absent until v2's catalog (v5) ships, so this is a no-op today.
    const v = tierVariantByBase(baseModel);
    if (v && v.contractAdapterFile) {
      const contract = arts.find((a) => a.kind === 'contract-adapter' && a.base_model === baseModel);
      if (!contract) {
        throw new Error("The contract adapter isn't available to download yet — please update the app.");
      }
      await downloadArtifact(contract);
    }
  } finally {
    // Always clear the in-flight flag + progress bar, even on a missing-artifact
    // throw or a download rejection — otherwise the button sticks on "Downloading…".
    state.dl.active = false;
    state.artDl = null;
    if (prog) prog.style.display = 'none';
  }
}

// B4: the hero installs through the SIGNED DISTRIBUTION CATALOG. Only when
// BOTH files have landed is the hero considered installed and the engine
// loaded — a base-only state is never treated as installed (fail-closed,
// mirroring inference::resolve_launch + download_status). The tier is whatever
// is effective now (auto/detected, or a pre-install override).
async function heroDownload(m, btn) {
  const el = $('errMsg');
  const fail = (msg) => {
    state.dl.active = false;
    state.artDl = null;
    const prog = $('prog'); if (prog) prog.style.display = 'none';
    if (btn) { btn.disabled = false; btn.textContent = 'Retry download'; }
    if (el) { el.style.display = 'block'; el.style.color = ''; el.textContent = msg; }
  };
  try {
    await downloadHeroPair(heroBaseModel(), btn);
    // Both files present → installed. finishInstalled loads the engine (which
    // launches base + adapter via --lora) and enters the chat.
    state.dl = { installed: true, partBytes: 0, active: false };
    await finishInstalled(m, btn);
  } catch (err) {
    fail(String(err && err.message ? err.message : err));
  }
}

// Wrapper tier-selection: change the hero's engine size. begin_tier_switch
// enforces the switch limit (offline); on approval we download the target
// pair if needed, then complete_tier_switch persists + relaunches the engine
// on it and sweeps the old pair off disk. A `noOp` (mode relabel, same model)
// just persists. Denials + errors surface on the drawer's errMsg line.
async function selectTier(m, mode) {
  if (state.tierSwitching) return; // re-entrancy guard: no overlapping switches
  state.tierSwitching = true;
  const el = $('errMsg');
  const btn = $('dlBtn');
  const showErr = (msg, muted) => {
    if (el) { el.style.display = 'block'; el.style.color = muted ? 'var(--muted)' : ''; el.textContent = msg; }
  };
  if (el) { el.style.display = 'none'; el.textContent = ''; }
  try {
    let plan;
    try {
      plan = await invoke('begin_tier_switch', { mode });
    } catch (err) {
      // A denial (the once-per-period limit) — informational, not a hard error.
      showErr(String(err && err.message ? err.message : err), true);
      return;
    }
    if (plan.needsDownload) {
      await downloadHeroPair(plan.baseModel, btn);
    }
    await invoke('complete_tier_switch', { mode });
    state.dl = { installed: true, partBytes: 0, active: false };
    state.tierSel = await invoke('get_tier_selection');
    // Re-render the drawer to reflect the new active tier + engine state.
    if (state.drawerId === m.id) openDrawer(m.id);
    renderGrid();
  } catch (err) {
    showErr(String(err && err.message ? err.message : err));
    try { state.tierSel = await invoke('get_tier_selection'); } catch (_) {}
    if (state.drawerId === m.id) openDrawer(m.id);
  } finally {
    state.tierSwitching = false;
  }
}

// B4: downloads ONE dist-catalog artifact and resolves only when that
// artifact's terminal `download-progress` event arrives (routed here by
// onDownloadProgress via state.artDl). `download_artifact` returns
// immediately (the work is on a Rust worker thread), so a resolved invoke is
// NOT completion — the promise settles on the done/failed/cancelled event, or
// on a synchronous invoke rejection (bad path / a download already running).
// An "Already installed." rejection means this artifact's file is already on
// disk (e.g. resuming after the base landed but the adapter failed) — treat
// it as success so the sequence proceeds to the next artifact.
function downloadArtifact(artifact) {
  return new Promise((resolve, reject) => {
    state.artDl = { path: artifact.path, resolve, reject };
    invoke('download_artifact', { artifact }).catch((err) => {
      // Only the synchronous validation rejections land here; a real download
      // failure arrives as a `failed` event handled in onDownloadProgress.
      if (!state.artDl || state.artDl.path !== artifact.path) return;
      state.artDl = null;
      const msg = String(err && err.message ? err.message : err);
      if (/already installed/i.test(msg)) resolve();
      else reject(new Error(msg));
    });
  });
}

async function finishInstalled(m, btn) {
  if (btn) { btn.disabled = true; btn.textContent = 'Starting engine…'; }
  try {
    await invoke('load_model', { modelId: m.id });
    renderGrid();
    enterChat(m);
  } catch (err) {
    if (btn) { btn.disabled = false; btn.textContent = 'Open chat'; }
    const el = $('errMsg');
    if (el) { el.style.display = 'block'; el.textContent = String(err); }
  }
}

function cancelPaymentPoll(resetBtnText, btnEl) {
  if (state.pay.timer) clearInterval(state.pay.timer);
  state.pay = { modelId: null, timer: null, deadline: 0, btnId: null };
  const btn = btnEl || $('dlBtn');
  if (btn && resetBtnText) { btn.disabled = false; btn.textContent = resetBtnText; }
  if (resetBtnText) {
    const el = $('errMsg');
    if (el) { el.style.display = 'none'; el.style.color = ''; }
  }
}

function beginPaymentPoll(m, btnId) {
  state.pay.modelId = m.id;
  state.pay.btnId = btnId || 'dlBtn';
  state.pay.deadline = Date.now() + 10 * 60 * 1000;
  state.pay.timer = setInterval(async () => {
    if (Date.now() > state.pay.deadline) {
      const drawerMatch = state.drawerId === m.id;
      const targetBtn = drawerMatch ? $(state.pay.btnId) : null;
      cancelPaymentPoll(drawerMatch ? idleLabel(m, targetBtn) : null, targetBtn);
      if (drawerMatch) {
        const el = $('errMsg');
        if (el) { el.style.display = 'block'; el.style.color = ''; el.textContent = "We didn't see a completed payment. If you paid, it will appear shortly — try Get again in a moment."; }
      }
      return;
    }
    let list;
    try { list = await invoke('list_entitlements'); } catch (_) { return; }
    const hit = (list || []).some((e) => e.modelId === m.id && e.source === 'purchase' && (!e.expiresAt || Date.parse(e.expiresAt) > Date.now()));
    if (!hit) return;
    cancelPaymentPoll(null);
    state.mine.add(m.id);
    state.lapsed.delete(m.id);
    state.chatBlocked.delete(m.id);
    renderGrid();
    const btn = $('dlBtn');
    if (btn && state.drawerId === m.id && $('drawer').classList.contains('show')) {
      const renewEl = $('renewLine');
      if (renewEl) renewEl.style.display = 'none';
      btn.disabled = true;
      if (state.dl.installed) {
        btn.textContent = '✓ Paid — starting engine…';
        finishInstalled(m, btn);
      } else {
        btn.textContent = '✓ Paid — starting download…';
        heroDownload(m, btn);
      }
    }
  }, 3000);
}

function fmtGiB(n) { return (n / 2 ** 30).toFixed(2); }

// Task 2.2: the engine-state row narrates downloads too, so a user sitting in
// the chat view is never left wondering whether anything is happening. This
// wraps the existing handler rather than editing it — the handler owns
// `state.dl`, has several early returns, and a `done` for ONE artifact of the
// base+adapter pair does not mean the hero is installed. Refreshing on the way
// in keeps the percentage live; refreshing on the way out picks up whatever
// `state.dl` settled to. The inner function is unchanged.
async function onDownloadProgress(e) {
  const p = e.payload;
  // `done` clears the slot so the row falls through to engine state rather
  // than latching on a finished download.
  state.dlProgress = p.phase === 'done' ? null : p;
  refreshEngineState();
  try {
    await onDownloadProgressInner(e);
  } finally {
    refreshEngineState();
  }
}

async function onDownloadProgressInner(e) {
  const p = e.payload;
  // B4: dist-catalog (two-artifact hero) sequence. download_artifact tags its
  // events with the artifact PATH as modelId (not a catalog id), so route the
  // one we're awaiting to the sequencing promise and keep the bar/line moving;
  // never fall through to the legacy single-file hero handling below.
  if (state.artDl && p.modelId === state.artDl.path) {
    const artBar = $('prog') ? $('prog').firstElementChild : null;
    const artLine = $('dlLine');
    if (p.phase === 'downloading' && p.totalBytes > 0) {
      state.dl.active = true;
      if (artBar) artBar.style.width = `${Math.floor((p.bytesDownloaded / p.totalBytes) * 100)}%`;
      if (artLine) {
        artLine.style.display = 'block';
        artLine.textContent = `${fmtGiB(p.bytesDownloaded)} / ${fmtGiB(p.totalBytes)} GiB · ${(p.bytesPerSec / 1e6).toFixed(1)} MB/s`;
      }
    } else if (p.phase === 'verifying') {
      if (artLine) { artLine.style.display = 'block'; artLine.textContent = 'Verifying download…'; }
      if (artBar) artBar.style.width = '100%';
    } else if (p.phase === 'done') {
      const co = state.artDl; state.artDl = null; co.resolve();
    } else if (p.phase === 'failed' || p.phase === 'cancelled') {
      const co = state.artDl; state.artDl = null;
      co.reject(new Error(p.error || (p.phase === 'cancelled' ? 'Download cancelled.' : 'Download failed.')));
    }
    return;
  }
  const hero = state.catalog.find((x) => x.id === p.modelId);
  const bar = $('prog') ? $('prog').firstElementChild : null;
  const line = $('dlLine');
  if (p.phase === 'downloading' && p.totalBytes > 0) {
    state.dl.active = true;
    if (bar) bar.style.width = `${Math.floor((p.bytesDownloaded / p.totalBytes) * 100)}%`;
    if (line) {
      line.style.display = 'block';
      line.textContent = `${fmtGiB(p.bytesDownloaded)} / ${fmtGiB(p.totalBytes)} GiB · ${(p.bytesPerSec / 1e6).toFixed(1)} MB/s`;
    }
  } else if (p.phase === 'verifying') {
    if (line) { line.style.display = 'block'; line.textContent = 'Verifying download…'; }
    if (bar) bar.style.width = '100%';
  } else if (p.phase === 'done') {
    state.dl = { installed: true, partBytes: 0, active: false };
    if (line) line.style.display = 'none';
    // The download outlives the session that started it (the file is
    // machine-global). If the CURRENT session isn't entitled to this model
    // — the starter signed out and someone else signed in mid-download —
    // record the machine fact and stop: no engine start, no auto-enter
    // into a paid chat.
    if (!state.signedIn || !hero || !state.mine.has(hero.id)) { renderGrid(); return; }
    const btn = $('dlBtn');
    if (btn) { btn.textContent = 'Starting engine…'; }
    try {
      await invoke('load_model', { modelId: p.modelId });
      if (hero) { state.mine.add(hero.id); renderGrid(); enterChat(hero); }
    } catch (err) {
      if (btn) { btn.disabled = false; btn.textContent = 'Open chat'; }
      const el = $('errMsg');
      if (el) { el.style.display = 'block'; el.textContent = String(err); }
    }
  } else if (p.phase === 'failed' || p.phase === 'cancelled') {
    state.dl.active = false;
    try {
      const ds = await invoke('download_status', { modelId: p.modelId });
      state.dl.partBytes = ds.partBytes;
      state.dl.installed = ds.installed;
    } catch (_) {}
    const btn = $('dlBtn');
    if (btn) {
      btn.disabled = false;
      btn.textContent = p.phase === 'failed' ? 'Retry download' : `Resume download · ${fmtGiB(state.dl.partBytes)} GiB so far`;
    }
    if (line) line.style.display = 'none';
    if (p.error) { const el = $('errMsg'); if (el) { el.style.display = 'block'; el.textContent = p.error; } }
  }
}

/* ---------------- knowledge packs ---------------- */
// #packBuildBtn stays disabled until #packName has a non-whitespace value —
// pure UX guard (build_personal_pack still accepts a null name server-side).
function syncPackBuildBtn() {
  const nameInput = $('packName');
  const buildBtn = $('packBuildBtn');
  if (buildBtn && nameInput) buildBtn.disabled = !nameInput.value.trim();
}

async function openPacksModal() {
  show($('packsModal'));
  syncPackBuildBtn();
  await refreshPacksList();
}

async function refreshPacksList() {
  const list = $('packsList');
  let packs;
  try {
    packs = await invoke('list_packs');
  } catch (e) {
    const err = $('packErr');
    if (err) { err.style.display = 'block'; err.textContent = String(e); }
    return;
  }
  if (!list) return;
  if (!packs.length) {
    list.innerHTML = `<div style="color:var(--muted);padding:14px 2px">No packs yet — build one from your notes.</div>`;
    return;
  }
  list.innerHTML = packs.map((p) => {
    const m = p.manifest;
    return `<div class="packrow">
      <div>
        <div>${escapeHtml(m.packId)}</div>
        <div class="packmeta">${escapeHtml(m.embeddingDims)}-dim · ${escapeHtml(m.chunkTargetTokens)}-tok chunks · built ${escapeHtml(m.builtBy)}</div>
      </div>
      <button class="btn" data-del="${escapeHtml(p.path)}">Delete</button>
    </div>`;
  }).join('');
}

async function startBuild() {
  let sel;
  try {
    sel = await window.__TAURI__.dialog.open({
      multiple: true,
      filters: [{ name: 'Documents (Markdown, text, PDF, HTML, Word, EPUB)', extensions: ['md', 'markdown', 'txt', 'pdf', 'html', 'htm', 'docx', 'epub'] }],
    });
  } catch (e) {
    const err = $('packErr');
    if (err) { err.style.display = 'block'; err.textContent = String(e); }
    return;
  }
  if (sel == null) return;
  const filePaths = Array.isArray(sel) ? sel : [sel];

  const prog = $('packProg'), bar = prog ? prog.firstElementChild : null;
  const line = $('packLine');
  const cancelBtn = $('packCancelBtn');
  const buildBtn = $('packBuildBtn');
  const nameInput = $('packName');
  const err = $('packErr');
  if (bar) bar.style.width = '0%';
  if (prog) prog.style.display = 'block';
  if (line) line.style.display = 'none';
  if (cancelBtn) cancelBtn.style.display = '';
  if (buildBtn) buildBtn.disabled = true;
  if (nameInput) nameInput.disabled = true;
  if (err) { err.style.display = 'none'; err.textContent = ''; }

  const name = nameInput ? (nameInput.value.trim() || null) : null;
  try {
    await invoke('build_personal_pack', { filePaths, name });
    if (prog) prog.style.display = 'none';
    if (line) line.style.display = 'none';
    if (cancelBtn) cancelBtn.style.display = 'none';
    if (nameInput) { nameInput.disabled = false; nameInput.value = ''; }
    syncPackBuildBtn(); // name just cleared — re-disable
    await refreshPacksList();
  } catch (e) {
    if (err) { err.style.display = 'block'; err.textContent = String(e); }
    if (prog) prog.style.display = 'none';
    if (cancelBtn) cancelBtn.style.display = 'none';
    if (nameInput) nameInput.disabled = false;
    syncPackBuildBtn(); // name is still whatever the user typed
  }
}

function onBuildProgress(e) {
  const p = e.payload;
  const prog = $('packProg');
  const bar = prog ? prog.firstElementChild : null;
  const line = $('packLine');
  // Scanned-PDF OCR phases (Task 4) — fired before 'parsing' for a PDF with
  // image-only pages. 'ocr-notice' is a one-shot soft-cap heads-up (no
  // page-count progress of its own); 'ocr' is per-page, so it also drives
  // the progress bar like 'embedding' below. Both carry a ready-made
  // `note` string from the backend — additive, the MD/TXT/PDF-with-text
  // phases below are unchanged.
  if (p.phase === 'ocr-notice') {
    if (line) { line.style.display = 'block'; line.textContent = p.note || `Scanned PDF: OCR'ing ${p.total} pages — this may take a few minutes.`; }
  } else if (p.phase === 'ocr') {
    if (line) { line.style.display = 'block'; line.textContent = p.note || `OCR page ${p.done}/${p.total}`; }
    if (bar && p.total > 0) bar.style.width = `${Math.floor((p.done / p.total) * 100)}%`;
  } else if (p.phase === 'parsing') {
    if (line) { line.style.display = 'block'; line.textContent = 'Reading your files…'; }
  } else if (p.phase === 'embedding') {
    if (line) { line.style.display = 'block'; line.textContent = `Embedding ${p.done}/${p.total} chunks`; }
    if (bar && p.total > 0) bar.style.width = `${Math.floor((p.done / p.total) * 100)}%`;
  } else if (p.phase === 'writing') {
    if (line) { line.style.display = 'block'; line.textContent = 'Finalizing pack…'; }
    if (bar) bar.style.width = '100%';
  }
  // 'done' is left to startBuild's success path, which resets the UI once
  // the build_personal_pack promise itself resolves.
}

/* ---------------- chat pack attachment (§3a A4) ---------------- */
async function openAttachModal() {
  show($('attachModal'));
  await renderAttachList();
}

async function renderAttachList() {
  const list = $('attachList');
  let packs;
  try {
    packs = await invoke('list_packs');
  } catch (e) {
    list.innerHTML = `<div style="color:var(--muted);padding:14px 2px">${escapeHtml(String(e))}</div>`;
    return;
  }
  if (!packs.length) {
    list.innerHTML = `<div style="color:var(--muted);padding:14px 2px">No packs yet — build one from the Packs button in your library, then attach it here.</div>`;
    return;
  }
  // Checkbox view onto the same list_packs data A3's picker uses — not a
  // delete view, so it's its own small renderer rather than a refactor of
  // refreshPacksList. packId/path come from filenames (untrusted): escape
  // both, especially inside the data-path attribute.
  list.innerHTML = packs.map((p) => {
    const checked = state.chat.packPaths.includes(p.path) ? 'checked' : '';
    return `<label class="attachrow"><input type="checkbox" data-path="${escapeHtml(p.path)}" ${checked}/> <span>${escapeHtml(p.manifest.packId)}</span></label>`;
  }).join('');
}

function updateGroundPill() {
  const pill = $('groundPill');
  if (state.chat.packPaths.length) {
    pill.hidden = false;
    pill.textContent = `Packs: ${state.chat.packPaths.length} attached`;
  } else {
    pill.hidden = true;
  }
}

function simulateStubDownload(m, btn) {
  const prog = $('prog'), bar = prog.firstElementChild;
  prog.style.display = 'block'; btn.disabled = true; btn.style.opacity = 0.7;
  let p = 0;
  const t = setInterval(() => {
    p += Math.random() * 22 + 8;
    bar.style.width = `${Math.min(p, 100)}%`;
    if (p >= 100) {
      clearInterval(t);
      state.mine.add(m.id);
      invoke('grant_entitlement', { modelId: m.id, source: 'library' }).catch(() => {});
      btn.innerHTML = 'Installed'; btn.disabled = false; btn.style.opacity = 1;
      $('installedMsg').style.display = 'flex';
      renderGrid();
    }
  }, 220);
}

/* ---------------- conversation sidebar + persistence (§7 S7-2) ---------------- */
// The sidebar lists CONVERSATIONS, never models (user-confirmed, permanent
// — see the task brief): one local model runs at a time, so every chat
// lives under it. A chat record is created lazily, on the first user
// message (see sendCompletion) — refreshChatList/openChat/newChat below
// are the sidebar's read/switch/create surface over that store.

async function refreshChatList() {
  let chats, folders;
  try {
    [chats, folders] = await Promise.all([invoke('list_chats'), invoke('list_folders')]);
  } catch (_) {
    return; // additive UI — leave whatever's already rendered on failure
  }
  state.sidebar.chats = chats;
  state.sidebar.folders = folders;
  // Keep an active search live through any mutation (move/archive/rename/
  // delete performed on a result row) — otherwise the result list would go
  // stale relative to state.sidebar.chats and the row's shown pinned/
  // archived/folder state could drift from what just happened.
  if (state.sidebar.searchResults != null && state.sidebar.query.trim()) {
    try { state.sidebar.searchResults = await invoke('search_chats', { query: state.sidebar.query }); } catch (_) {}
  }
  renderSidebar();
}

// title is user-renameable (untrusted) — escapeHtml it; pin/rename/delete/
// archive/move/export are static labels, not interpolated user data.
// `opts.nested` indents a row under a folder header.
function chatRowHtml(c, opts) {
  const nested = opts && opts.nested;
  const moveOpen = state.sidebar.moveMenuFor === c.id;
  const exportOpen = state.sidebar.exportMenuFor === c.id;
  return `
    <div class="chatrow ${c.id === state.chat.chatId ? 'active' : ''}${nested ? ' nested' : ''}${c.archived ? ' is-archived' : ''}" data-id="${c.id}">
      <span class="chatrow-title">${escapeHtml(c.title)}</span>
      <span class="chatrow-actions">
        <button class="chatrow-act" data-act="pin" data-pinned="${c.pinned ? '1' : '0'}" title="${c.pinned ? 'Unpin' : 'Pin'}">${c.pinned ? '★' : '☆'}</button>
        <button class="chatrow-act" data-act="archive" data-archived="${c.archived ? '1' : '0'}" title="${c.archived ? 'Unarchive' : 'Archive'}">${c.archived ? '⤒' : '⤓'}</button>
        <button class="chatrow-act" data-act="rename" title="Rename">✎</button>
        <span class="movemenu-wrap">
          <button class="chatrow-act" data-act="move" title="Move to…">⇄</button>
          <div class="movemenu ${moveOpen ? 'show' : ''}">
            <button class="movemenu-item" data-act="move-to" data-folder-id="">Unfiled</button>
            ${state.sidebar.folders.map((f) => `<button class="movemenu-item" data-act="move-to" data-folder-id="${f.id}">${escapeHtml(f.name)}</button>`).join('')}
          </div>
        </span>
        <span class="movemenu-wrap">
          <button class="chatrow-act" data-act="export" title="Export…">⇩</button>
          <div class="movemenu ${exportOpen ? 'show' : ''}">
            <button class="movemenu-item" data-act="export-to" data-format="markdown">Markdown</button>
            <button class="movemenu-item" data-act="export-to" data-format="json">JSON</button>
            <button class="movemenu-item" data-act="export-to" data-format="txt">Plain text</button>
          </div>
        </span>
        <button class="chatrow-act" data-act="delete" title="Delete">✕</button>
      </span>
    </div>`;
}

// name is user-renameable (untrusted) — escapeHtml it. The header row is a
// collapse toggle (click anywhere but the ⋯ menu → expand/collapse); a folder's
// chats render only when expanded (default collapsed) to save vertical space.
// `count` (a number) is shown when collapsed so hidden chats stay discoverable.
function folderHeaderHtml(f, count) {
  const menuOpen = state.sidebar.folderMenuFor === f.id;
  const expanded = state.sidebar.expandedFolders.has(f.id);
  return `
    <div class="folder-header ${expanded ? 'expanded' : 'collapsed'}" data-folder-id="${f.id}" data-folder-act="collapse-toggle">
      <span class="folder-chevron">${expanded ? '▾' : '▸'}</span>
      <span class="folder-name">${escapeHtml(f.name)}</span>
      ${!expanded && count ? `<span class="folder-count">${count}</span>` : ''}
      <span class="folder-menu-wrap">
        <button class="chatrow-act" data-folder-act="toggle" title="Folder options">⋯</button>
        <div class="movemenu ${menuOpen ? 'show' : ''}">
          <button class="movemenu-item" data-folder-act="rename">Rename</button>
          <button class="movemenu-item" data-folder-act="delete">Delete</button>
        </div>
      </span>
    </div>`;
}

// The grouped view: each folder (sortOrder, then id) as a header + its
// chats, then an "Unfiled" section for folderId==null — only when at least
// one folder exists, so a fresh account with no folders renders exactly as
// plain S7-2 did (no redundant "Unfiled" label over the whole list). A live
// search instead renders state.sidebar.searchResults flat, ignoring both
// grouping and the archived filter (ChatGPT/Claude-style: search finds
// across everything). Archived chats are hidden from the grouped view
// unless "Show archived" is toggled on, in their own trailing section.
function renderSidebar() {
  const list = $('chatList');
  const footBtn = $('showArchivedBtn');
  if (footBtn) footBtn.textContent = state.sidebar.showArchived ? 'Hide archived' : 'Show archived';

  if (state.sidebar.searchResults != null) {
    const results = state.sidebar.searchResults;
    list.innerHTML = results.length
      ? results.map((c) => chatRowHtml(c)).join('')
      : `<div class="chatlist-empty">No matches.</div>`;
    return;
  }

  const chats = state.sidebar.chats;
  if (!chats.length) {
    list.innerHTML = `<div class="chatlist-empty">No conversations yet.</div>`;
    return;
  }
  const active = chats.filter((c) => !c.archived);
  const archived = chats.filter((c) => c.archived);
  const folders = [...state.sidebar.folders].sort((a, b) => a.sortOrder - b.sortOrder || a.id - b.id);

  let html = '';
  if (folders.length) {
    for (const f of folders) {
      const inFolder = active.filter((c) => c.folderId === f.id);
      html += folderHeaderHtml(f, inFolder.length);
      if (state.sidebar.expandedFolders.has(f.id)) {
        html += inFolder.length
          ? inFolder.map((c) => chatRowHtml(c, { nested: true })).join('')
          : `<div class="folder-empty">No chats</div>`;
      }
    }
    const unfiled = active.filter((c) => c.folderId == null);
    html += `<div class="folder-header unfiled"><span class="folder-name">Unfiled</span></div>`;
    html += unfiled.length
      ? unfiled.map((c) => chatRowHtml(c, { nested: true })).join('')
      : `<div class="folder-empty">No chats</div>`;
  } else if (active.length) {
    html += active.map((c) => chatRowHtml(c)).join('');
  } else {
    html += `<div class="chatlist-empty">All conversations are archived — Show archived below.</div>`;
  }

  if (state.sidebar.showArchived) {
    html += `<div class="folder-header archived-section"><span class="folder-name">Archived</span></div>`;
    html += archived.length
      ? archived.map((c) => chatRowHtml(c)).join('')
      : `<div class="folder-empty">No archived chats</div>`;
  }

  list.innerHTML = html;
}

// Debounced (~150ms) query -> search_chats, guarded against an empty/
// whitespace query (never calls search_chats with one — matches the
// grouped view instead) and against out-of-order resolution: if the query
// has moved on by the time this resolves, its result is discarded.
let chatSearchDebounce = null;
function onChatSearchInput(q) {
  state.sidebar.query = q;
  clearTimeout(chatSearchDebounce);
  if (!q.trim()) {
    state.sidebar.searchResults = null;
    renderSidebar();
    return;
  }
  chatSearchDebounce = setTimeout(() => runChatSearch(q), 150);
}
async function runChatSearch(q) {
  let results;
  try { results = await invoke('search_chats', { query: q }); } catch (_) { return; }
  if (state.sidebar.query !== q) return; // stale — a newer query has since landed
  state.sidebar.searchResults = results;
  renderSidebar();
}
function clearChatSearch() {
  $('chatSearch').value = '';
  state.sidebar.query = '';
  state.sidebar.searchResults = null;
  clearTimeout(chatSearchDebounce);
  renderSidebar();
}

// Clears the message DOM back to just the model's greeting, without
// touching state.chat.chatId — callers (newChat, delete-active-chat,
// enterChat on a model switch) each decide what chatId should be first.
function resetChatDom() {
  state.chat.messages = [];
  rebuildChatDom();
  updateGroundPill();
}

async function openChat(id) {
  // Switching chats must not let an in-flight stream for the OLD chat keep
  // running against the NEW one's DOM/state — abort it first (finishStream
  // still persists whatever partial content it already has, to the chat it
  // actually belongs to; see sendCompletion/finishStream's turnChatId).
  state.chat.aborter?.abort();
  let detail;
  try {
    detail = await invoke('get_chat', { id });
  } catch (_) {
    return; // failed open is a no-op — the current chat stays as-is
  }
  const { chat, messages } = detail;
  state.chat.chatId = chat.id;
  state.chat.packPaths = chat.mountedPacks || [];
  // Re-hydrate into the same {role, content, citations?} shape sendMessage/
  // finishStream push locally, so rebuildChatDom's replay logic (below)
  // doesn't need to know whether a message came from the DB or this session.
  state.chat.messages = messages.map((msg) => ({
    role: msg.role,
    content: msg.content,
    // The row id, so a reopened chat's replies can still be acted on — the
    // health worker's route confirmation (Task 8) is written against it.
    id: msg.id,
    // Phase 2: a supervised reply's verdict and the confirmed route, both
    // absent on every ordinary message (convstore skips them when null), so a
    // tutor chat re-hydrates exactly as it did before.
    guard: msg.guard || undefined,
    confirmedRoute: msg.confirmedRoute || undefined,
    citations: msg.citations && msg.citations.length ? msg.citations : undefined,
    // §3a/Task 7: the `messages.tool_calls` column, surfaced camelCase (via
    // convstore's MessageInfo `#[serde(rename_all = "camelCase")]`) as
    // `msg.toolCalls` — NOT `msg.tool_calls`. Same shape finishStream stashed
    // it in: `[{expression, display}]` or `[{expression, error}]`.
    calculations: msg.toolCalls && msg.toolCalls.length ? msg.toolCalls : undefined,
  }));
  rebuildChatDom();
  updateGroundPill();
  closeSidebarDrawer(); // mobile: picking a chat is what the drawer is for
  await refreshChatList(); // active-row highlight moves to this chat
}

// The "+ New chat" button: eagerly creates a fresh record (unlike the
// lazy first-message create below) inheriting the current model + packs,
// same as ChatGPT/Claude's "new chat" always producing a visible row.
async function newChat() {
  // Same reasoning as openChat: never leave a stream running against a
  // chat that's about to stop being the active one.
  state.chat.aborter?.abort();
  const m = state.chat.model;
  let chatId = null;
  try {
    const chat = await invoke('create_chat', {
      title: 'New chat',
      folderId: null,
      mountedPacks: state.chat.packPaths,
      modelId: m?.id ?? '',
      // B4 provenance: stamp the hero's always-on behavioral adapter id when
      // the model declares one (empty otherwise — the historical value).
      adapterIds: heroAdapterId(m) ? [heroAdapterId(m)] : [],
    });
    chatId = chat.id;
  } catch (_) { /* persistence failed — still hand back a clean local chat */ }
  state.chat.chatId = chatId;
  resetChatDom();
  await refreshChatList();
}

/* ---------------- chat ---------------- */

// §7 S7-4 / D-4: fit each request to the window the engine ACTUALLY has.
// Policy and arithmetic live in `context-window.js`, where they are tested at
// both shipping window sizes; the window itself is read from the engine via
// `engineWindow(state.engine)`, because the previous version of this comment
// said "must match inference.rs `-c`" and was true right up until mobile got a
// per-tier window that nothing propagated. See that module's header.
// Appended to the system prompt on UNGROUNDED turns (no packs attached this
// turn). Without grounding the base model will otherwise parrot/fabricate
// "source titles" from earlier grounded turns still in the transcript — the
// grounded path is hardened symmetrically in retrieve.rs. Interim mitigation;
// the contract-trained LoRA adapter is the real fix for grounding-honesty.
const UNGROUNDED_NO_SOURCES_NOTE = ' No documents are attached to this conversation, so you have no sources to cite. Do not list, cite, or invent source titles; if asked about your sources, say none are attached.';
function enterChat(m) {
  // A pack attached in one chat shouldn't silently carry into another
  // model's chat (e.g. a medical pack leaking into an education chat).
  // Re-entering the SAME model's chat keeps the attachment AND the active
  // persisted chat (chatId), mirroring how state.chat.messages already
  // persists across exit/re-enter — only a genuine model switch resets to
  // a clean, unsaved chat (chatId=null; the first message persists it
  // lazily, same as a brand-new session — see sendCompletion).
  if (state.chat.model && state.chat.model.id !== m.id) {
    state.chat.packPaths = [];
    state.chat.chatId = null;
    state.chat.messages = [];
  }
  state.chat.model = m;
  $('chatModelName').textContent = m.name;
  $('chatCover').src = m.coverUrl;
  rebuildChatDom();
  updateGroundPill();
  refreshChatList();
  closeDrawer();
  const views = $('views');
  views.classList.add('in-chat');
  views.addEventListener('transitionend', function onEnd(e) {
    if (e.propertyName !== 'transform') return;
    views.removeEventListener('transitionend', onEnd);
    $('chatInput').focus();
  });
  setTimeout(() => { if (views.classList.contains('in-chat')) $('chatInput').focus(); }, 650);
}

function exitChat() {
  state.chat.aborter?.abort();
  $('views').classList.remove('in-chat');
}

function rebuildChatDom() {
  const m = state.chat.model;
  const box = $('chatMessages');
  box.innerHTML = '';
  appendBubble('assistant', m.greeting || 'Hi!');
  // A stored assistant message's .citations (set by finishStream on a
  // grounded turn) is replayed here so citations don't vanish when the
  // chat is exited and re-entered. .noEvidence messages carry no citations
  // and render like any other bubble — their content IS the refusal text.
  // .calculations (Task 7) replays the same way, from `messages.tool_calls`.
  // Phase 2: in a supervised chat an assistant row with NO verdict is withheld
  // — see `replayMessage`. That covers a partial row left by a killed turn, a
  // row whose `attach_guard` never landed, and any other producer that did not
  // pass through the guard. The greeting bubble above is UI, not a stored
  // message, and is deliberately outside this loop.
  const supervised = chatIsSupervised({ supervised: !!(m && m.supervised), messages: state.chat.messages });
  for (const msg of state.chat.messages) {
    const view = replayMessage({ supervised, role: msg.role, content: msg.content, guard: msg.guard });
    // A withheld reply shows nothing of its own, its sources and its
    // calculations included: they are provenance for text that is not on
    // screen.
    appendBubble(
      msg.role, view.text,
      view.withheld ? undefined : msg.citations,
      view.withheld ? undefined : msg.calculations,
      view.banner,
    );
  }
  updateContextDivider();
}

// The route banner: product-owned text, above the model's words and never
// inside them. `provisional` is the one drawn from the streamed prefix before
// the reply is finished — same copy, visibly unfinished, replaced by the
// verdict's own banner at `finishStream`.
//
// The key is normalised once and used for BOTH the copy and the CSS class, so
// a banner can never render a disposition it is not coloured as.
function bannerEl(banner, provisional = false) {
  const key = bannerKey(banner);
  const b = bannerText(key);
  const el = document.createElement('div');
  el.className = `triage-banner triage-banner--${key}${provisional ? ' triage-banner--provisional' : ''}`;
  const title = document.createElement('b');
  title.textContent = b.title;
  el.append(title);
  // The `unverified` banner is a title and nothing else; every route banner
  // carries a line. Appending an empty span would leave a stray space in a
  // `pre-wrap` bubble.
  if (b.line) {
    const line = document.createElement('span');
    line.textContent = ` ${b.line}`;
    el.append(line);
  }
  return el;
}

// `banner` is a banner KEY (Phase 2) — one of the four routes, or the
// `unverified` one. Absent on every tutor turn and on every user turn, which is
// what keeps this function's behaviour for those byte-identical to what it was.
function appendBubble(role, text, citations, calculations, banner) {
  const el = document.createElement('div');
  el.className = `msg ${role}`;
  el.textContent = text;
  if (banner) el.prepend(bannerEl(banner));
  $('chatMessages').appendChild(el);
  if (citations && citations.length) renderCitations(el, citations);
  if (calculations && calculations.length) renderCalculations(el, calculations);
  $('chatMessages').scrollTop = $('chatMessages').scrollHeight;
  return el;
}

// §7 S7-4: mark the boundary where older messages fall outside the
// request window — they're still saved (transcript + DB), just not sent to
// the model. Only meaningful for the ACTIVE chat's rendered transcript and
// only at rest (never mid-stream, since bubbles are still being appended/
// mutated then) — callers are rebuildChatDom (on chat open) and
// finishStream (after a completed turn is pushed, for the still-active
// chat). Idempotent: always removes any existing divider first so it's
// safe to call repeatedly as the chat grows.
function updateContextDivider() {
  const box = $('chatMessages');
  box.querySelector('.ctx-divider')?.remove();
  const m = state.chat.model;
  if (!m) return;
  // View-time approximation: we can't know if the NEXT turn will be
  // grounded (which would shrink the window further via a larger system
  // prompt), so this uses the plain systemPrompt as an honest baseline,
  // not a guarantee.
  const { droppedCount } = windowMessages(state.chat.messages, m.systemPrompt, m.greeting, engineWindow(state.engine));
  if (droppedCount <= 0) return;
  // index 0 of .msg is the greeting bubble; indices 1.. map 1:1 to
  // state.chat.messages, so messageBubbles[droppedCount] is the first
  // bubble still inside the window.
  const messageBubbles = [...box.querySelectorAll('.msg')].slice(1);
  const boundary = messageBubbles[droppedCount];
  if (!boundary) return; // DOM/state out of sync for any reason — no-op, never throw
  const divider = document.createElement('div');
  divider.className = 'ctx-divider';
  divider.textContent = '⌇ Older messages are saved but aren\'t in the model\'s memory';
  boundary.before(divider);
}

// §3a A4: citations render via DOM textContent, never innerHTML — docTitle/
// sectionPath/locator come from user-attached pack content (untrusted),
// so textContent avoids any markup-injection risk without needing escaping.
// §7 S7-2: the list is collapsed by default behind a disclosure toggle —
// grounded turns can carry many sources and they'd otherwise eat the chat.
// Re-rendered the same way whether it's a live turn (sendCompletion/
// finishStream) or a persisted one replayed by rebuildChatDom on openChat.
function renderCitations(afterEl, citations) {
  const box = document.createElement('div');
  box.className = 'citations';
  const toggle = document.createElement('button');
  toggle.type = 'button';
  toggle.className = 'citetoggle';
  const label = (open) => `${open ? '⌃' : '⌄'} ${citations.length} source${citations.length === 1 ? '' : 's'}`;
  toggle.textContent = label(false);
  toggle.addEventListener('click', () => {
    const open = box.classList.toggle('expanded');
    toggle.textContent = label(open);
  });
  box.appendChild(toggle);
  const list = document.createElement('div');
  list.className = 'citelist';
  for (const c of citations) {
    const row = document.createElement('div');
    row.className = 'cite';
    // Main line: the numbered document + where in it. textContent only —
    // docTitle/sectionPath/locator come from user pack content (untrusted).
    const main = document.createElement('div');
    main.className = 'cite-main';
    main.textContent = `[${c.n}] ${c.docTitle}` +
      (c.sectionPath ? ` · ${c.sectionPath}` : '') +
      (c.locator ? ` · ${c.locator}` : '');
    row.appendChild(main);
    // Which PACK this excerpt is from — the same label as the attach-modal
    // checkbox (manifest packId), so the user can reconcile the sources
    // against exactly what they attached. Retrieval is scoped to the
    // attached packs; this makes that visible (a doc can live in more than
    // one pack, and one pack can contribute several excerpts).
    if (c.packId) {
      const pk = document.createElement('div');
      pk.className = 'cite-pack';
      pk.textContent = `pack: ${c.packId}`;
      row.appendChild(pk);
    }
    list.appendChild(row);
  }
  box.appendChild(list);
  afterEl.insertAdjacentElement('afterend', box);
  $('chatMessages').scrollTop = $('chatMessages').scrollHeight;
  return box;
}

// Mirrors renderCitations immediately above (§3a A4's visual grammar: a
// collapsed-by-default disclosure panel under the bubble, "here's where
// this came from") for the calc() tool's provenance — expressions/results
// are model-generated text, so textContent only, never innerHTML. Each row
// is `expression = display` for a clean eval, or `expression → error` when
// the tool call failed (kpack-calc's domain/parse errors — see calc-tool.js).
function renderCalculations(afterEl, calcs) {
  const box = document.createElement('div');
  box.className = 'calculations';
  const toggle = document.createElement('button');
  toggle.type = 'button';
  toggle.className = 'calctoggle';
  const label = (open) => `${open ? '⌃' : '⌄'} ${calcs.length} calculation${calcs.length === 1 ? '' : 's'}`;
  toggle.textContent = label(false);
  toggle.addEventListener('click', () => {
    const open = box.classList.toggle('expanded');
    toggle.textContent = label(open);
  });
  box.appendChild(toggle);
  const list = document.createElement('div');
  list.className = 'calclist';
  for (const c of calcs) {
    const row = document.createElement('div');
    row.className = 'calc';
    row.textContent = c.error ? `${c.expression} → ${c.error}` : `${c.expression} = ${c.display}`;
    list.appendChild(row);
  }
  box.appendChild(list);
  afterEl.insertAdjacentElement('afterend', box);
  $('chatMessages').scrollTop = $('chatMessages').scrollHeight;
  return box;
}

function setComposerEnabled(on) {
  $('chatInput').disabled = !on;
  $('sendBtn').disabled = !on || state.chat.streaming;
}

// The engine banner is normally a plain status line. `action` (optional) turns
// it into an actionable one — a trailing button — so a recoverable failure
// (a missing model file) carries its own fix instead of a dead instruction.
function showEngineBanner(text, action) {
  const b = $('engineBanner');
  b.hidden = false;
  b.textContent = text; // wipes any prior action button
  if (action) {
    const btn = document.createElement('button');
    btn.type = 'button';
    btn.className = 'banner-action';
    btn.textContent = action.label;
    btn.onclick = action.onClick;
    b.appendChild(btn);
  }
}
function hideEngineBanner() { $('engineBanner').hidden = true; }

/* ---------------- engine state, said out loud (task 2.2) ----------------

   Before this, `#chatStatusPill` was referenced exactly once in the entire
   codebase — in index.html, as the hard-coded string "Local · offline" —
   and no code ever read or wrote it. The founder "could not identify the
   engine-state element" because there wasn't one; the only thing that ever
   reported engine state was `#engineBanner`, on restart and failure only, so
   the ordinary Starting → Ready path rendered nothing at all.

   The mapping and the pacing live in `engine-state.js` (pure, tested —
   decision D-3: logic whose failure mode is silent goes where the tests
   run). Everything below is DOM writes.

   MOBILE ONLY. On desktop `refreshEngineState` returns before touching
   anything, so the pill keeps its static string and desktop behaviour is
   byte-identical. Desktop's pill is the same defect and is surfaced as
   desktop-scope backlog rather than fixed here, because fixing it is a
   desktop behaviour change this task is not allowed to make.               */

const engineSeq = createReadableSequence({
  now: () => Date.now(),
  schedule: (fn, ms) => setTimeout(fn, ms),
  cancel: (h) => clearTimeout(h),
  render: renderEngineState,
});
let prefillTimer = null;

function renderEngineState(p) {
  const row = $('engineState');
  const pill = $('chatStatusPill');
  // H1 invariant: every path here is textContent. None of this copy is model
  // output, but the rule is about the sink, not the source — a new renderer
  // that reaches for innerHTML is the regression to look for.
  $('engineStateLabel').textContent = p.label;
  const detail = $('engineStateDetail');
  detail.textContent = p.detail || '';
  detail.hidden = !p.detail;
  row.className = 'enginestate tone-' + p.tone;
  row.dataset.kind = p.kind;
  row.hidden = !p.prominent;

  // Exactly one engine-state element is visible at a time: the prominent row
  // while something is happening, the chat-bar pill once there is nothing to
  // announce. Showing both would duplicate the same sentence.
  // `short`, not `label`: the pill has ~170px beside the model name, and the
  // row has the width of the screen. Sharing one string is what rendered
  // "Ready — r" — the pill's rect was inside the viewport, so a bounds
  // assertion passed while the text was still cut.
  pill.hidden = p.prominent;
  pill.textContent = '';
  const dot = document.createElement('span');
  dot.className = 'pill-dot';
  pill.append(dot, document.createTextNode(p.short || p.label));

  // An actionable state carries its own control. A row that says "download
  // stopped partway" with no way to resume is the same defect as the engine
  // banner that said "re-download it" without a button — the instruction is
  // useless where the user is standing. `p.action` is a NAME; the mapping to a
  // handler lives here so engine-state.js stays DOM-free.
  const oldBtn = row.querySelector('.enginestate-action');
  if (oldBtn) oldBtn.remove();
  if (p.action) {
    const hero = heroEntry();
    if (hero) {
      const btn = document.createElement('button');
      btn.type = 'button';
      btn.className = 'enginestate-action';
      btn.textContent = p.action === 'resume' ? 'Resume' : 'Download';
      btn.onclick = () => { btn.disabled = true; heroDownload(hero, btn); };
      row.appendChild(btn);
    }
  }

  const prog = $('engineStateProg');
  const dl = p.kind === 'downloading' && state.dlProgress && state.dlProgress.totalBytes > 0;
  prog.hidden = !dl;
  if (dl) {
    const frac = state.dlProgress.bytesDownloaded / state.dlProgress.totalBytes;
    prog.firstElementChild.style.width = Math.min(100, frac * 100).toFixed(1) + '%';
  }
}

/// Gather what is true right now and offer it to the sequence.
///
/// Cheap and idempotent, so every event that could change the answer just
/// calls it rather than each one working out what to display.
function refreshEngineState() {
  if (!IS_MOBILE) return;
  const p = describeEngineState({
    engineStatus: state.engine && state.engine.status,
    download: state.dlProgress,
    installed: state.dl.installed,
    // Bytes already on disk from a download that stopped. `download_status`
    // has always reported these; until now the only place they surfaced was
    // the model drawer's "Resume download" button, which a user sitting in the
    // chat view never sees.
    partBytes: state.dl.partBytes,
    downloadActive: state.dl.active,
    turn: state.turn ? { ...state.turn, now: Date.now() } : null,
    supported: state.hw ? state.hw.supported !== false : true,
    thermal: state.thermal,
    now: Date.now(),
  });
  engineSeq.push(p);
}

/// A turn began. The wait for the first token IS prefill, and on the floor
/// tier the first turn of a conversation is genuinely slow (CP1: "first turn
/// very slow, subsequent turns lightning fast"), so the escalation to an
/// explanation is armed here rather than guessed at later.
function markTurnStarted() {
  state.turn = { startedAt: Date.now(), firstDeltaAt: null };
  clearTimeout(prefillTimer);
  prefillTimer = setTimeout(refreshEngineState, PREFILL_EXPLAIN_MS + 30);
  refreshEngineState();
}
function markFirstDelta() {
  if (!state.turn || state.turn.firstDeltaAt != null) return;
  state.turn.firstDeltaAt = Date.now();
  clearTimeout(prefillTimer);
  refreshEngineState();
}
function markTurnEnded() {
  state.turn = null;
  clearTimeout(prefillTimer);
  refreshEngineState();
}

/* ---------------- the conversation drawer (task 2.2) ----------------

   The measured root cause of every chat-layout complaint from the field:
   `#chatSidebar{width:260px;flex:none}` took 72.2% of a 360px viewport and
   left the chat column 100px. As an overlay the rail costs the chat nothing
   when closed. Desktop keeps the static two-column layout untouched.       */

function setDrawer(open) {
  if (!IS_MOBILE) return;
  $('chatSidebar').classList.toggle('open', open);
  $('sidebarScrim').classList.toggle('show', open);
  $('chatMenuBtn').setAttribute('aria-expanded', String(open));
}
function closeSidebarDrawer() { setDrawer(false); }

/// The honest "not yet" screen (2.3 produces `supported:false`, 2.2 renders
/// it). Deliberately NOT a hard block: the library, the chats already on the
/// device and the account still work — the claim is only that a model cannot
/// run here, which is the truth and the whole truth.
function showNotYetScreen(hw) {
  const ram = hw && typeof hw.ram_gb === 'number' ? hw.ram_gb : null;
  $('notYetWhy').textContent = ram != null
    ? `This device reports ${ram} GB of memory.`
    : 'This device does not report enough memory.';
  $('notYetScreen').hidden = false;
}

function pulseCost() {
  const c = $('costCounter');
  c.textContent = '$0.00';
  c.classList.remove('pulse');
  void c.offsetWidth; // restart animation
  c.classList.add('pulse');
}

function sendMessage() {
  const input = $('chatInput');
  const text = input.value.trim();
  if (!text || state.chat.streaming) return;
  input.value = '';
  state.chat.messages.push({ role: 'user', content: text });
  appendBubble('user', text);
  sendCompletion(text);
}

// `userText` is only set when called from sendMessage — retry chips (below)
// call sendCompletion() bare to replay an already-pushed/already-persisted
// user turn, which must NOT be persisted a second time.
async function sendCompletion(userText) {
  // Streaming guard set SYNCHRONOUSLY, before any await below — otherwise a
  // second rapid Send could slip past sendMessage's `state.chat.streaming`
  // check (read synchronously there) and race a second sendCompletion call
  // (and a second create_chat below). Same reasoning for the fresh
  // AbortController: it needs to be in place before any await so a chat
  // switch (openChat/newChat) can abort THIS turn even if the switch
  // happens while create_chat is still in flight.
  state.chat.streaming = true;
  $('sendBtn').hidden = true; $('stopBtn').hidden = false;
  state.chat.aborter = new AbortController();
  markTurnStarted(); // starts the prefill clock; see engine-state.js

  // Wrapper tier-selection: the first chat commits the current engine size —
  // after this, tier changes are limited to once per billing period. Fire the
  // (idempotent) Rust latch ONCE per session, then refresh the cached selector
  // state so the drawer reflects the new limit; reset the guard on failure so a
  // later send retries.
  if (!state.tierCommitted) {
    state.tierCommitted = true;
    invoke('mark_tier_committed')
      .then(() => invoke('get_tier_selection'))
      .then((ts) => { state.tierSel = ts; })
      .catch(() => { state.tierCommitted = false; });
  }

  // §7 S7-5: detect the first exchange SYNCHRONOUSLY (before any await
  // below) — state.chat.messages is this (the ACTIVE) chat's transcript,
  // and the just-pushed user message is present with no assistant reply
  // for this turn yet, so `.some(assistant)` is false ONLY on the very
  // first exchange. `userText == null` (a retry chip replaying an
  // already-pushed turn) is never treated as a first exchange. The
  // captured `userText` (the source for the title call) travels with this
  // turn via `finishStream`'s `autoTitle` param, same as `turnChatId`.
  const isFirstExchange = userText != null && !state.chat.messages.some((msg) => msg.role === 'assistant');
  const autoTitle = isFirstExchange ? { source: userText } : null;

  const m = state.chat.model;
  // Phase 2, and the gate for every guard behaviour below. Captured here, with
  // the entry itself, because `enterChat` can swap the model mid-stream without
  // aborting the turn: whether this reply is guarded is decided by the entry it
  // was SENT under, never by whatever the library is showing when it lands.
  const supervised = !!(m && m.supervised);
  document.querySelectorAll('.retrychip').forEach((el) => el.remove());
  const bubble = appendBubble('assistant', '');
  bubble.classList.add('streaming');
  let acc = '';
  // The provisional route banner, and the clock it is measured against. The
  // model states its disposition in the first clause, so this reaches the
  // screen well before the reply finishes.
  const sentAt = performance.now();
  let provisionalEl = null;

  // Persistence (§7 S7-2): lazily create the chat record on the very first
  // user message. `turnChatId` is captured ONCE for this turn and threaded
  // through (into finishStream/the noEvidence branch below) rather than
  // re-read from state.chat.chatId later — switching chats (openChat/
  // newChat/the sidebar row-click/deleting the active chat) aborts this
  // stream AND can repoint state.chat.chatId at a different chat while
  // create_chat is still in flight, so re-reading it after an await risks
  // misfiling this turn into the wrong chat. Wrapped in try/catch so a save
  // failure degrades silently — it never blocks or breaks the streaming
  // path below.
  let turnChatId = state.chat.chatId;
  if (userText != null) {
    if (turnChatId == null) {
      try {
        const chat = await invoke('create_chat', {
          title: userText.split('\n')[0].slice(0, 40).trim() || 'New chat',
          folderId: null,
          mountedPacks: state.chat.packPaths,
          modelId: m?.id ?? '',
          // B4 provenance: the hero's always-on behavioral adapter id (empty
          // for models that declare none — the historical value).
          adapterIds: heroAdapterId(m) ? [heroAdapterId(m)] : [],
        });
        turnChatId = chat.id;
        // Only adopt it as the app's ACTIVE chat if nothing else claimed
        // that slot while create_chat was in flight — a chat switch sets
        // state.chat.chatId synchronously before its own await, so if
        // it's non-null here someone else already won the race.
        if (state.chat.chatId == null) {
          state.chat.chatId = turnChatId;
          refreshChatList();
        }
      } catch (_) { turnChatId = null; /* not saved — chat keeps working locally */ }
    }
    if (turnChatId != null) {
      invoke('append_message', { chatId: turnChatId, role: 'user', content: userText }).catch(() => {});
    }
  }

  // §3a A4: when packs are attached to this chat, ground the turn through
  // rag_query BEFORE touching the model. When no packs are attached this
  // whole block is skipped and everything below runs exactly as it did
  // before A4 — same fetch, same SSE parsing, same system message.
  //
  // Phase 2: NEVER for a supervised entry, however many packs are attached.
  // Grounding replaces the one thing Task 5 pins — the catalog's systemPrompt
  // as the only system content — and it carries a second unguarded producer
  // with it: the scripted `noEvidence` refusal below is written into the
  // transcript and persisted without ever passing through `applyGuard`.
  let groundedPrompt = null, groundedCitations = null;
  if (shouldGroundTurn({ supervised, packCount: state.chat.packPaths.length })) {
    const query = state.chat.messages[state.chat.messages.length - 1]?.content;
    if (query != null) {
      bubble.textContent = 'Searching your packs…';
      let rag;
      try {
        rag = await invoke('rag_query', { query, packPaths: state.chat.packPaths });
      } catch (e) {
        // If Stop was hit during the pack search, honor it: bail silently
        // rather than surfacing a "pack search failed" retry chip for a turn
        // the user deliberately cancelled.
        // No `turn` argument: this bail-out carries no content, so it reaches
        // no guard, no persist and no title. Grounding is off for a supervised
        // entry anyway, so this line is unreachable from one.
        if (state.chat.aborter.signal.aborted) { finishStream(bubble, '', null, [], turnChatId); return; }
        // Do NOT silently fall through to an ungrounded send — that would
        // betray the "this answer cites your packs" promise. Fail the turn
        // instead, same shape as the existing fetch-failure retry chip.
        bubble.remove();
        state.chat.streaming = false;
        $('sendBtn').hidden = false; $('stopBtn').hidden = true;
        $('sendBtn').disabled = false;
        const retry = document.createElement('button');
        retry.className = 'retrychip';
        retry.textContent = `⟳ Pack search failed (${String(e)}) — tap to retry`;
        retry.onclick = () => { retry.remove(); sendCompletion(); };
        $('chatMessages').appendChild(retry);
        $('chatMessages').scrollTop = $('chatMessages').scrollHeight;
        return;
      }
      // §3a A4-fix: rag_query isn't tied to the abort signal, so a Stop
      // clicked during "Searching your packs…" resolves here rather than
      // cancelling the IPC. Honor it — drop the turn without committing a
      // refusal or a grounded answer to history or the pill.
      // Same as above: no content, so no `turn` is needed and none is passed.
      if (state.chat.aborter.signal.aborted) { finishStream(bubble, '', null, [], turnChatId); return; }
      if (rag.status === 'noEvidence') {
        // Adapter v2: when the contract adapter is composed on this tier, it is
        // trained to emit the fixed refusal-with-offer on the [[NO_EVIDENCE]]
        // marker — so hand the marker to the model and let IT refuse. Only when
        // no contract adapter is present do we fall back to the interim scripted
        // refusal (the un-adapted base model won't reliably refuse on its own).
        // Gate on the SAME field the engine composes + verifies the adapter on
        // (contractAdapterFile — inference.rs / downloadHeroPair), not
        // contractAdapterId: if a catalog ever declared the id without the file,
        // keying off the id would hand [[NO_EVIDENCE]] to a NON-contract-composed
        // base model that won't reliably refuse — fail-open toward hallucination.
        const contractActive = tierVariant(effectiveTier())?.contractAdapterFile;
        if (!contractActive) {
          const n = state.chat.packPaths.length;
          const refusal = `I couldn't find anything about that in your attached pack${n > 1 ? 's' : ''}, so I won't guess. Try rephrasing, or attach a pack that covers it.`;
          // §7 S7-2: only touch the live DOM/transcript if this turn's chat is
          // STILL active — the user may have switched away mid rag_query.
          const isActive = turnChatId === state.chat.chatId;
          bubble.classList.remove('streaming');
          if (isActive) {
            bubble.textContent = refusal;
            state.chat.messages.push({ role: 'assistant', content: refusal, noEvidence: true });
          }
          if (turnChatId != null) {
            invoke('append_message', { chatId: turnChatId, role: 'assistant', content: refusal })
              .then(refreshChatList).catch(() => {});
          }
          state.chat.streaming = false;
          $('sendBtn').hidden = false; $('stopBtn').hidden = true;
          $('sendBtn').disabled = false;
          if (isActive) {
            const pill = $('groundPill');
            pill.hidden = false;
            pill.textContent = 'No evidence in your packs';
          }
          // Returns without finishStream, so a no-evidence first turn never
          // auto-titles — the first-line title stands.
          return; // no model call — no inference ran
        }
        // Contract adapter composed: send rag.prompt (contract + [[NO_EVIDENCE]])
        // and let the adapter produce the refusal. Fall through to streaming.
        groundedPrompt = rag.prompt;
        groundedCitations = [];
        bubble.textContent = '';
        const pill = $('groundPill');
        pill.hidden = false;
        pill.textContent = 'No evidence in your packs';
      } else {
        // grounded: swap this turn's system message for the assembled grounded
        // prompt (contract + numbered sources) and continue into streaming.
        groundedPrompt = rag.prompt;
        groundedCitations = rag.citations;
        bubble.textContent = '';
        const pill = $('groundPill');
        pill.hidden = false;
        pill.textContent = `Grounded in ${groundedCitations.length} source${groundedCitations.length === 1 ? '' : 's'}`;
      }
    }
  }

  try {
    // §7 S7-4: fit the request to n_ctx — send only the most-recent
    // messages that fit the budget left after the fixed preamble (system +
    // greeting) and the reply reserve. For a grounded turn the (large)
    // groundedPrompt is part of `sys`, so its length correctly shrinks the
    // history budget — sources + kept history still stay within n_ctx.
    // Short chats are unaffected: windowMessages returns the whole list
    // (droppedCount 0), so behavior is byte-identical to before.
    // `fingerprint` is read ONLY inside the supervised branch of
    // `assembleMessages`, where it re-hashes the prompt about to be sent and
    // throws if it is not the prompt the catalog pins — the gate was run under
    // that exact text, so a drifted one is a different model wearing the same
    // name. The tutor entry declares no fingerprint and never reaches the
    // check. The throw is caught below and shown as an engine banner: a
    // supervised turn that cannot prove its prompt is not sent at all.
    const { system: sys } = assembleMessages({
      entry: m, groundedPrompt, sent: [], ungroundedNote: UNGROUNDED_NO_SOURCES_NOTE,
      fingerprint: promptFingerprint,
    });
    const win = windowMessages(state.chat.messages, sys, m.greeting, engineWindow(state.engine));
    // Assembled again with the windowed history now that `sys` (and thus the
    // budget it leaves for history) is known — see windowMessages above. For
    // a supervised entry this drops the greeting turn entirely; its tokens
    // were still budgeted by windowMessages (a few dozen tokens of slack).
    const assembled = assembleMessages({
      entry: m, groundedPrompt, sent: win.sent, ungroundedNote: UNGROUNDED_NO_SOURCES_NOTE,
      fingerprint: promptFingerprint,
    });
    // One turn, described once for both platforms (task 2.1). On desktop this
    // lands in `calc-loop.js`, which owns the fetch/SSE-parse/tool-execute/
    // resubmit cycle end to end and is Tauri/DOM-free by design; on Android it
    // lands in `chat_stream`, where the same loop runs Rust-side because every
    // round needs the live session. `tools`/`runCalc` are meaningful only to
    // the first and are ignored by the second — see transport.js.
    //
    // `onDelta` keeps `acc` growing exactly as the old inline loop did, so the
    // AbortError branch below still sees whatever partial text streamed before
    // the abort.
    const out = await transport.streamTurn({
      port: state.engine.port,
      // The chat this turn belongs to — mobile's KV-reuse key, so consecutive
      // turns of one conversation share a warm prefix (task 1.5). Desktop
      // ignores it; `cache_prompt: true` is how the sidecar does the same job.
      chatId: turnChatId,
      baseBody: { max_tokens: REPLY_RESERVE, temperature: 0.7, cache_prompt: true }, // bound to the windowing reserve so the two can't drift
      messages: assembled.messages,
      tools: [CALC_TOOL],
      runCalc: (expression) => invoke('calc', { expression }),
      onDelta: (d) => {
        markFirstDelta(); // the wait before this delta was prefill — see engine-state.js
        acc += d;
        // Show the leading-`<think></think>`-stripped view every render
        // (idempotent — see stripLeadingThink); `acc` keeps the raw text
        // so the strip decision is always re-made against the full prefix.
        const view = stripLeadingThink(acc);
        bubble.textContent = view;
        // Supervised only. `provisionalEl` IS the state machine's
        // `alreadyShown`, so once a route resolves `provisionalStep` returns
        // early and never reads the prefix again — the detectors read the whole
        // of it, which would otherwise be work per token for an answer that
        // cannot change. The tutor is stopped by the gate before the call.
        if (supervised) {
          const step = provisionalStep({ supervised, alreadyShown: !!provisionalEl, prefixText: view, elapsedMs: performance.now() - sentAt });
          if (step.banner) {
            provisionalEl = bannerEl(step.banner, true);
            console.log(step.log);
          }
        }
        // Re-attached rather than rebuilt: the `textContent` write above
        // replaces every child of the bubble, banner included.
        if (provisionalEl) bubble.prepend(provisionalEl);
        $('chatMessages').scrollTop = $('chatMessages').scrollHeight;
      },
      signal: state.chat.aborter.signal,
    });
    finishStream(bubble, out.content, groundedCitations, out.calculations, turnChatId, autoTitle,
      { entry: m, userText, messageId: out.messageId });
  } catch (err) {
    if (err.name === 'AbortError') {
      finishStream(bubble, acc, groundedCitations, [], turnChatId, autoTitle, { entry: m, userText, messageId: null });
      return;
    }
    bubble.remove();
    state.chat.streaming = false;
    markTurnEnded();
    $('sendBtn').hidden = false; $('stopBtn').hidden = true;
    // A supervised entry whose system prompt is not the one its catalog
    // fingerprint was cut from never reached the model — `assembleMessages`
    // threw before the send. That is not a transport failure, so it gets no
    // retry chip: retrying re-throws, and the fix is a correct catalog, not a
    // second attempt. Loud banner, composer disabled, nothing sent.
    if (isPromptMismatch(err)) {
      showEngineBanner('This model\'s prompt is not the one it was checked with, so nothing was sent. Reinstall the model.');
      setComposerEnabled(false);
      return;
    }
    try {
      const info = await invoke('engine_info');
      state.engine = info;
      refreshEngineState();
      if (info.status === 'NoModel') { showEngineBanner('Model not downloaded yet.'); setComposerEnabled(false); }
      else if (info.status !== 'Ready') { showEngineBanner('Local engine restarting…'); setComposerEnabled(false); }
    } catch (_) {}
    const retry = document.createElement('button');
    retry.className = 'retrychip';
    retry.textContent = '⟳ That didn\'t go through — tap to retry';
    retry.onclick = () => { retry.remove(); sendCompletion(); };
    $('chatMessages').appendChild(retry);
    $('chatMessages').scrollTop = $('chatMessages').scrollHeight;
  }
}

// `turnChatId` is the chat THIS turn belongs to, captured once at
// turn-start in sendCompletion (never re-read from state.chat.chatId,
// which may have moved on to a different chat by the time this runs — see
// the sendCompletion comment). Synchronous top to bottom: the shared
// composer state (streaming/Send/Stop/pulseCost) always restores
// immediately; the DB write is fire-and-forget and never gates it.
// `autoTitle` (§7 S7-5) is `{ source: userText }` on a first exchange, or
// null — also fire-and-forget, threaded through the same way as
// `turnChatId` so a mid-stream chat switch still titles the RIGHT chat.
// `calculations` ([{expression, display}] or [{expression, error}] from
// streamWithTools, `[]`/undefined on abort/partial paths) is stashed on the
// pushed message + rendered via renderCalculations, and persisted to the
// `messages.tool_calls` column below (Task 7) — same treatment as
// `citations` throughout this function.
//
// `turn` (Phase 2) is what this turn was SENT with, threaded through for the
// same reason as `turnChatId`: none of it may be re-read from `state` here.
//   entry      the catalog entry — `enterChat` can swap models mid-stream
//              without aborting, and whether this reply is guarded must not
//              depend on which model the library is showing when it lands;
//   userText   the user's words for this turn, which is what the crisis check
//              reads (a retry chip replays an already-pushed turn and passes
//              null, so the transcript is consulted instead);
//   messageId  the assistant row `chat_cmds.rs::settle` already finalized on
//              mobile, or null. See `persistAssistantTurn`.
// Absent on the two rag_query bail-outs, which pass no content and so reach
// none of it.
function finishStream(bubble, acc, citations, calculations, turnChatId, autoTitle, turn) {
  bubble.classList.remove('streaming');
  markTurnEnded(); // the turn is over on every path through here, abort included
  // Only touch the live DOM/in-memory transcript if this turn's chat is
  // STILL the active one — the user may have switched chats mid-stream
  // (which aborts this turn's fetch, via openChat/newChat, but whatever
  // partial `acc` had already accumulated still gets here). A turn whose
  // chat is no longer active must not repaint over whatever chat is on
  // screen now; it still gets PERSISTED to its own chat below.
  const isActive = turnChatId === state.chat.chatId;
  // B4: the persisted + in-memory content is the leading-`<think></think>`-
  // stripped view, matching what was shown in the bubble during streaming.
  // A turn that was ONLY an empty think block strips to '' and is treated
  // exactly like an empty turn (bubble removed, nothing persisted).
  const shown = stripLeadingThink(acc);

  // ---- Phase 2: the product's contract on a supervised reply --------------
  // The guard runs on `shown` — AFTER the think strip, never on `acc` — and
  // keeps the raw reply inside the verdict, so what the model said survives
  // whatever is removed from the screen. Everything below is `null` for the
  // tutor, and every branch that reads it is a no-op there.
  const entry = (turn && turn.entry) || state.chat.model;
  const supervised = !!(entry && entry.supervised);
  // The user turn the crisis check reads: this turn's own words when it is a
  // fresh send, else the last user message (a retry chip replays a pushed one).
  const userTurn = turn && turn.userText != null
    ? turn.userText
    : ([...state.chat.messages].reverse().find((x) => x.role === 'user')?.content ?? '');
  const verdict = supervised && shown
    ? applyGuard({ userText: userTurn, replyText: shown, crisisLine: entry.crisisLine || undefined })
    : null;
  const display = verdict ? verdict.displayText : shown;
  if (verdict) {
    console.log(`[triage] route ${verdict.route} banner ${verdict.banner} stripped ${verdict.timeframeStripped.length} crisis ${verdict.crisisLineAppended}`);
    if (isActive) {
      // The final banner always replaces the provisional one. Writing
      // `textContent` clears every child, the provisional banner included, so
      // the prepend below is what puts the verdict's own banner up — and there
      // is never a moment with two.
      bubble.textContent = display;
      bubble.prepend(bannerEl(verdict.banner));
    }
  }

  if (isActive) {
    if (shown) {
      const msg = { role: 'assistant', content: display };
      // The verdict travels with the message so `rebuildChatDom` can replay the
      // banner, and so Task 8's confirmation UI has the route to confirm.
      if (verdict) msg.guard = verdict;
      // Stash citations on the pushed message (not just rendered here) so
      // rebuildChatDom can replay them if the chat is exited and re-entered.
      if (citations && citations.length) {
        msg.citations = citations;
        renderCitations(bubble, citations);
      }
      // Same treatment for the calc() tool's provenance (Task 7) — stashed
      // on `msg` so rebuildChatDom's replay picks it up via `msg.calculations`.
      if (calculations && calculations.length) {
        msg.calculations = calculations;
        renderCalculations(bubble, calculations);
      }
      state.chat.messages.push(msg);
    } else {
      bubble.remove();
    }
  }
  state.chat.streaming = false;
  $('sendBtn').hidden = false; $('stopBtn').hidden = true;
  $('sendBtn').disabled = false;
  pulseCost();
  // §7 S7-4: re-check the context-window divider now that the turn is
  // pushed and streaming has ended for THIS chat. `isActive` (above) is
  // exactly `turnChatId === state.chat.chatId`; `state.chat.streaming` was
  // just set false on the line above, so this only ever runs at rest, for
  // the chat actually on screen — never mid-stream, never on a chat the
  // user has switched away from.
  if (isActive && !state.chat.streaming) updateContextDivider();
  // §7 S7-2: persist the completed turn to the chat it actually belongs
  // to. `turnChatId` is only null here if the earlier create_chat in
  // sendCompletion failed — in that case this turn silently isn't saved
  // either (same degrade-gracefully contract). Fire-and-forget: the
  // composer state above must never wait on this DB write.
  //
  // WHICH command, and why it is not always `append_message`: on mobile the
  // streaming checkpoint row is finalized by `chat_cmds.rs::settle` BEFORE the
  // `done` event, so a guarded turn is ATTACHED to that row rather than
  // appended as a second one. `persistAssistantTurn` decides; the tutor's call
  // is the same `append_message` with the same arguments it always made.
  const persist = persistAssistantTurn({
    chatId: turnChatId,
    content: display,
    citations,
    calculations,
    guard: verdict,
    messageId: turn ? turn.messageId : null,
  });
  //
  // AND A FAILED PERSIST IS NOT ALWAYS SILENT. A failed `append_message` is —
  // nothing was written and nothing is left behind, which is the app's existing
  // degrade-gracefully contract. A failed `attach_guard` is not: the row is
  // already there, holding the model's RAW reply, so saying nothing would
  // persist the unguarded text and show it on the next open. Retried once, then
  // said out loud. `persistFailurePlan` owns that decision and is tested.
  if (persist) {
    const attempt = (n) => invoke(persist.command, persist.args).then((info) => {
      // The row id, kept on the in-memory message: it is what the health
      // worker's route confirmation is written against (Task 8).
      if (isActive && info && info.id) {
        const last = state.chat.messages[state.chat.messages.length - 1];
        if (last && last.role === 'assistant' && last.content === display) last.id = info.id;
      }
      refreshChatList(); // updated_at bump reorders the sidebar
    }).catch((e) => {
      const plan = persistFailurePlan({ command: persist.command, attempt: n, error: e });
      if (plan.action === 'ignore') return;
      console.log(plan.log);
      if (plan.action === 'retry') { attempt(n + 1); return; }
      // The bubble is left exactly as it is. `display` was computed here, from
      // this reply, and is still what the guard decided to show — the failure
      // is that it was not RECORDED, not that it cannot be trusted.
      showEngineBanner(plan.message);
    });
    attempt(1);
  }
  // §7 S7-5: fire-and-forget the title for this turn — do NOT await it (it
  // must never gate the composer restore above, which already ran).
  // turnChatId-scoped like the persist above, so a mid-stream chat switch
  // still titles the right chat.
  //
  // A supervised chat is NOT titled by `maybeAutoTitle`: that makes a second
  // completion, with its own system prompt at temperature 0.3, against a model
  // whose whole contract is that it only ever sees the pinned prompt at
  // temperature 0. It is titled from the user's own first message instead.
  if (shown && turnChatId != null && autoTitle) {
    const titling = titlePlan({ supervised, source: autoTitle.source });
    if (titling.kind === 'model') {
      maybeAutoTitle(turnChatId, autoTitle.source, shown);
    } else if (titling.kind === 'fixed') {
      // Same command, so the same guard applies: a chat the user has already
      // renamed keeps its name (`title_auto`).
      invoke('auto_title_chat', { id: turnChatId, title: titling.title })
        .then((applied) => { if (applied === true) refreshChatList(); })
        .catch(() => {});
    }
  }
}

// §7 S7-5: after the first complete exchange in a NEW chat, ask the local
// model for a concise 3–6 word title and apply it — unless the user has
// already renamed the chat (auto_title_chat's title_auto guard handles
// that server-side; this function never has to know). Self-contained and
// never throws into its caller (finishStream calls it fire-and-forget):
// every failure mode here (engine not ready, timeout, bad response, IPC
// error) just leaves the first-line title in place.
async function maybeAutoTitle(chatId, userText, assistantText) {
  if (!state.engine?.port || state.engine.status !== 'Ready') return;
  try {
    const aborter = new AbortController();
    const timer = setTimeout(() => aborter.abort(), 10000);
    let raw;
    try {
      raw = await transport.complete({
        port: state.engine.port,
        messages: [
          { role: 'system', content: 'You write very short chat titles. Reply with ONLY a 3–6 word title for the conversation. No quotes, no trailing punctuation, no preamble, no "Title:".' },
          { role: 'user', content: `User: ${userText.slice(0, 500)}\nAssistant: ${assistantText.slice(0, 500)}\n\nTitle:` },
        ],
        maxTokens: 24,
        temperature: 0.3,
        signal: aborter.signal,
      });
    } finally {
      clearTimeout(timer);
    }
    if (!raw) return;
    // Sanitize: strip the hero adapter's leading empty <think></think>
    // (B4 — this hits the same --lora-loaded engine as every streamed turn,
    // and the strip must run BEFORE the first-line split: Qwen3 may emit the
    // tags on their own lines, which would otherwise make the whole title
    // "<think>"), then first line only, strip an echoed "Title:" prefix,
    // strip surrounding quotes/backticks/asterisks, collapse whitespace,
    // trim, strip trailing punctuation, cap length.
    let title = stripLeadingThink(raw).split('\n')[0];
    title = title.replace(/^\s*title\s*:\s*/i, '');
    title = title.replace(/^[\s"'`*]+|[\s"'`*]+$/g, '');
    title = title.replace(/\s+/g, ' ').trim();
    title = title.replace(/[.,:;]+$/, '').trim();
    title = title.slice(0, 60);
    if (!title) return; // keep the first-line title
    const applied = await invoke('auto_title_chat', { id: chatId, title });
    if (applied === true) refreshChatList(); // sidebar row + active-row highlight update
  } catch (_) {
    // Network/abort/parse failure — the first-line title stands, invisibly.
  }
}

/* ---------------- signed-out nudge ---------------- */
let toastTimer = null;
function showToast(text) {
  let t = document.getElementById('toast');
  if (!t) {
    t = document.createElement('div');
    t.id = 'toast';
    t.className = 'toast';
    t.addEventListener('click', () => { t.classList.remove('show'); show($('loginModal')); });
    document.body.appendChild(t);
  }
  t.textContent = text;
  t.classList.add('show');
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => t.classList.remove('show'), 2600);
}
function lockNudge(card) {
  card.classList.add('locked');
  setTimeout(() => card.classList.remove('locked'), 900);
  showToast('Log in or create an account to open this model — tap here to log in.');
}

/* ---------------- auth ---------------- */
function show(el) { el.classList.add('show'); const i = el.querySelector('input'); if (i) setTimeout(() => i.focus(), 50); }
function hide(el) {
  el.classList.remove('show');
  // Any dismissal of a modal (close button, backdrop, Escape) clears its
  // typed-but-abandoned transient state — it must not sit in the DOM for
  // the next open. loginModal/createModal clear their forms; packsModal
  // clears the build-name field + any build error; attachModal clears its
  // (unsubmitted, since Done already copied checked boxes into
  // state.chat.packPaths) checkbox list, which renderAttachList rebuilds
  // fresh from state on the next open anyway.
  if (el.id === 'loginModal' || el.id === 'createModal') resetAuthForms();
  else if (el.id === 'packsModal') {
    $('packName').value = '';
    syncPackBuildBtn(); // name just cleared — re-disable
    const err = $('packErr');
    err.style.display = 'none';
    err.textContent = '';
  } else if (el.id === 'attachModal') {
    $('attachList').innerHTML = '';
  }
}

function openProfileMenu() {
  $('profileMenu').classList.add('show');
  $('profileBtn').setAttribute('aria-expanded', 'true');
}
function closeProfileMenu() {
  $('profileMenu').classList.remove('show');
  $('profileBtn').setAttribute('aria-expanded', 'false');
}

function resetAuthForms() {
  $('li-email').value = '';
  $('li-pass').value = '';
  $('li-remember').checked = false;
  $('cr-email').value = '';
  $('cr-pass').value = '';
  $('cr-nick').value = '';
  $('cr-remember').checked = false;
  $('li-err').style.display = 'none';
  $('cr-err').style.display = 'none';
  resetPwStrength();
}

async function applySession(info) {
  state.signedIn = true;
  state.nick = info.nickname || 'you';
  state.mine = new Set((info.entitlements || []).map((e) => e.modelId));
  state.lapsed = new Set((info.entitlements || [])
    .filter((e) => e.source === 'purchase' && e.expiresAt && Date.parse(e.expiresAt) < Date.now())
    .map((e) => e.modelId));
  state.chatBlocked = new Set((info.entitlements || [])
    .filter((e) => e.source === 'purchase' && e.expiresAt && Date.parse(e.expiresAt) + LAPSE_GRACE_MS < Date.now())
    .map((e) => e.modelId));
  try {
    state.device = await invoke('detect_hardware');
    $('dev-name').textContent = state.device.gpu.replace(/NVIDIA |GeForce /g, '') || 'This machine';
    $('dev-spec').textContent = `${state.device.ram_gb} GB RAM`;
    $('device').style.display = 'flex';
  } catch (_) { /* chip is optional — badges fall back to 'Sign in to check' */ }
  $('loginBtn').style.display = 'none';
  $('profileWrap').style.display = '';
  $('profileBtn').textContent = (state.nick || '?').trim().charAt(0).toUpperCase() || '?';
  $('profileBtn').title = info.mode === 'offlineCached' ? 'Signed in — offline, using saved account data' : 'Account menu';
  $('profileNick').textContent = state.nick;
  $('packsBtn').style.display = '';
  hide($('loginModal'));
  hide($('createModal'));
  resetAuthForms();
  maybeShowConsent(info);
  renderGrid();
}

/* ---------------- events ---------------- */
$('nav').addEventListener('click', (e) => {
  const b = e.target.closest('button[data-cat]'); if (!b) return;
  document.querySelectorAll('#nav button').forEach((x) => x.classList.remove('active'));
  b.classList.add('active');
  state.cat = b.dataset.cat; state.subject = 'all';
  renderFilters(); renderGrid();
});
$('subjectFilters').addEventListener('click', (e) => {
  const b = e.target.closest('button[data-subject]'); if (!b) return;
  state.subject = b.dataset.subject; renderFilters(); renderGrid();
});
$('search').addEventListener('input', (e) => { state.q = e.target.value; renderGrid(); });
$('grid').addEventListener('click', (e) => {
  const c = e.target.closest('.card');
  if (!c) return;
  if (!state.signedIn) { lockNudge(c); return; }
  openDrawer(c.dataset.id);
});
$('scrim').addEventListener('click', closeDrawer);
$('drawer').addEventListener('click', (e) => { if (e.target.closest('[data-close]')) closeDrawer(); });
document.addEventListener('keydown', (e) => {
  if (e.key === 'Escape') {
    closeDrawer(); hide($('loginModal')); hide($('createModal')); hide($('packsModal')); hide($('attachModal')); hide($('removeAccountModal')); closeProfileMenu();
    if (state.sidebar.moveMenuFor != null || state.sidebar.folderMenuFor != null) {
      state.sidebar.moveMenuFor = null; state.sidebar.folderMenuFor = null; renderSidebar();
    }
  }
});
$('loginBtn').addEventListener('click', () => show($('loginModal')));
$('profileBtn').addEventListener('click', (e) => {
  e.stopPropagation();
  $('profileMenu').classList.contains('show') ? closeProfileMenu() : openProfileMenu();
});
document.addEventListener('click', (e) => {
  if (!$('profileMenu').classList.contains('show')) return;
  if (e.target.closest('#profileWrap')) return;
  closeProfileMenu();
});
document.querySelectorAll('[data-close]').forEach((b) => b.addEventListener('click', () => { hide($('loginModal')); hide($('createModal')); hide($('packsModal')); hide($('attachModal')); hide($('removeAccountModal')); }));
$('packsBtn').addEventListener('click', () => openPacksModal());
$('attachPacksBtn').addEventListener('click', () => openAttachModal());
$('attachDoneBtn').addEventListener('click', async () => {
  const checked = $('attachList').querySelectorAll('input[type=checkbox]:checked');
  state.chat.packPaths = [...checked].map((cb) => cb.dataset.path);
  hide($('attachModal'));
  updateGroundPill();
  // §7 S7-2: persist the new attachment set so a reopened chat restores its
  // grounding. If no chat exists yet, it's captured at create time instead.
  if (state.chat.chatId != null) {
    try { await invoke('set_chat_packs', { id: state.chat.chatId, mountedPacks: state.chat.packPaths }); } catch (_) {}
  }
});
$('packBuildBtn').addEventListener('click', () => startBuild());
$('packName').addEventListener('input', () => syncPackBuildBtn());
$('packCancelBtn').addEventListener('click', async () => {
  try { await invoke('cancel_build'); } catch (_) {}
});
$('packsList').addEventListener('click', async (e) => {
  const btn = e.target.closest('[data-del]');
  if (!btn) return;
  const err = $('packErr');
  if (err) { err.style.display = 'none'; err.textContent = ''; }
  try {
    await invoke('delete_pack', { path: btn.dataset.del });
    await refreshPacksList();
  } catch (e2) {
    if (err) { err.style.display = 'block'; err.textContent = String(e2); }
  }
});
$('toCreate').addEventListener('click', () => { hide($('loginModal')); show($('createModal')); });
$('toLogin').addEventListener('click', () => { hide($('createModal')); show($('loginModal')); });
$('doLogin').addEventListener('click', async () => {
  const email = $('li-email').value.trim();
  const password = $('li-pass').value;
  const err = $('li-err');
  err.style.display = 'none';
  if (!email || !password) {
    err.style.display = 'block';
    err.textContent = 'Enter your email and password.';
    return;
  }
  const btn = $('doLogin');
  btn.disabled = true;
  btn.textContent = 'Signing in…';
  try {
    await applySession(await invoke('sign_in', { email, password, remember: $('li-remember').checked }));
  } catch (e) {
    err.style.display = 'block';
    err.textContent = String(e);
  } finally {
    btn.disabled = false;
    btn.textContent = 'Log in';
  }
});
$('doCreate').addEventListener('click', async () => {
  const email = $('cr-email').value.trim();
  const password = $('cr-pass').value;
  const err = $('cr-err');
  err.style.display = 'none';
  if (!email || !password) {
    err.style.display = 'block';
    err.textContent = 'Enter your email and password.';
    return;
  }
  if (!state.crStrengthOk) {
    err.style.display = 'block';
    err.textContent = 'Choose a stronger password — at least 12 characters (a passphrase of a few words works well).';
    return;
  }
  const btn = $('doCreate');
  btn.disabled = true;
  btn.textContent = 'Creating…';
  try {
    await applySession(await invoke('sign_up', { email, password, nickname: $('cr-nick').value.trim() || 'you', remember: $('cr-remember').checked }));
  } catch (e) {
    err.style.display = 'block';
    err.textContent = String(e);
  } finally {
    btn.disabled = false;
    btn.textContent = 'Create account';
  }
});
// Offline-auth (Task 7): live password-strength meter on the sign-up form.
// The offline verifier makes weak passwords brute-forceable with device
// access, so account creation is gated on the backend zxcvbn policy
// (length >= 12 AND score >= 3). state.crStrengthOk mirrors the last check;
// the Create button stays disabled until it passes.
let crStrengthTimer = null;
function refreshCreateEnabled() {
  const emailOk = $('cr-email').value.trim().length > 0;
  $('doCreate').disabled = !(state.crStrengthOk && emailOk);
}
function resetPwStrength() {
  state.crStrengthOk = false;
  $('cr-strength').querySelectorAll('.pwmeter-bar span').forEach((b) => { b.className = ''; });
  $('cr-strength-tip').textContent = 'Use at least 12 characters — a passphrase of a few words works well.';
  $('doCreate').disabled = true;
}
function updatePwStrength() {
  const pw = $('cr-pass').value;
  if (!pw) { resetPwStrength(); return; }
  // Invalidate SYNCHRONOUSLY, before the debounce: the Create button must
  // never trust an `ok` left over from a previous, stronger value. Without
  // this, an edit — or a password manager's autofill-then-Enter — inside the
  // 150ms window could submit a weak password, which enroll_verifier would
  // turn into a brute-forceable offline verifier. Safety over 150ms latency.
  state.crStrengthOk = false;
  $('doCreate').disabled = true;
  clearTimeout(crStrengthTimer);
  crStrengthTimer = setTimeout(async () => {
    let res;
    try {
      res = await invoke('check_password_strength', {
        password: pw, email: $('cr-email').value.trim(), nickname: $('cr-nick').value.trim(),
      });
    } catch (_) { return; }
    if ($('cr-pass').value !== pw) return; // a stale in-flight result — ignore
    state.crStrengthOk = !!res.ok;
    const score = Math.max(0, Math.min(4, res.score | 0));
    const filled = Math.max(1, score);
    $('cr-strength').querySelectorAll('.pwmeter-bar span').forEach((b, i) => {
      b.className = i < filled ? `on s${score}` : '';
    });
    $('cr-strength-tip').textContent = (res.feedback && res.feedback[0])
      || (res.ok ? 'Looks good.' : 'Too weak — use 12+ characters or a longer passphrase.');
    refreshCreateEnabled();
  }, 150);
}
['cr-pass', 'cr-email', 'cr-nick'].forEach((id) => $(id).addEventListener('input', updatePwStrength));
resetPwStrength(); // Create starts disabled until a strong password is entered

// Offline-auth (Task 7): one-time disclosure that an offline sign-in
// credential is stored on this device. Enrollment happens backend-side on
// any online auth regardless; this is the user-facing notice + the pointer
// to removal. Shown once (persisted flag), only after a genuine ONLINE auth.
function maybeShowConsent(info) {
  if (!info || info.mode !== 'online') return;
  try { if (localStorage.getItem('cleophis.offlineAuthConsent')) return; } catch (_) { return; }
  show($('consentModal'));
}
$('consentAgree').addEventListener('click', () => {
  try { localStorage.setItem('cleophis.offlineAuthConsent', '1'); } catch (_) {}
  hide($('consentModal'));
});

// Offline-auth (Task 7): "Remove account from this device" — the current
// account only (the backend resolves current_user_id; the FE never supplies
// a user id). On success the account + its session are gone, so reload to a
// clean signed-out state (or whichever remembered account still restores).
$('removeAccountBtn').addEventListener('click', () => {
  closeProfileMenu();
  $('removeWipeData').checked = false;
  $('remove-err').style.display = 'none';
  show($('removeAccountModal'));
});
$('removeCancel').addEventListener('click', () => hide($('removeAccountModal')));
$('removeConfirm').addEventListener('click', async () => {
  const btn = $('removeConfirm');
  const err = $('remove-err');
  err.style.display = 'none';
  btn.disabled = true;
  btn.textContent = 'Removing…';
  try {
    await invoke('remove_current_account_from_device', { wipeLocalData: $('removeWipeData').checked });
    window.location.reload();
  } catch (e) {
    err.style.display = 'block';
    err.textContent = String(e);
    btn.disabled = false;
    btn.textContent = 'Remove account';
  }
});

[$('loginModal'), $('createModal'), $('packsModal'), $('attachModal'), $('removeAccountModal')].forEach((md) => md.addEventListener('click', (e) => { if (e.target === md) hide(md); }));
$('chatBack').addEventListener('click', () => exitChat());
// Task 2.2 (mobile): the conversation rail as a drawer. Inert on desktop —
// #chatMenuBtn is display:none there and setDrawer returns immediately.
$('chatMenuBtn').addEventListener('click', () => {
  setDrawer(!$('chatSidebar').classList.contains('open'));
});
$('sidebarScrim').addEventListener('click', closeSidebarDrawer);
$('notYetBrowse').addEventListener('click', () => { $('notYetScreen').hidden = true; });
// A phone's Back gesture should close the drawer before leaving the chat.
window.addEventListener('keydown', (e) => {
  if (e.key === 'Escape' && $('chatSidebar').classList.contains('open')) closeSidebarDrawer();
});
$('sendBtn').addEventListener('click', () => sendMessage());
$('chatInput').addEventListener('keydown', (e) => { if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); sendMessage(); } });
$('stopBtn').addEventListener('click', () => state.chat.aborter?.abort());
$('newChatBtn').addEventListener('click', () => newChat());
$('newFolderBtn').addEventListener('click', async () => {
  const name = window.prompt('New folder name');
  if (name == null || !name.trim()) return;
  try { await invoke('create_folder', { name: name.trim() }); } catch (_) {}
  await refreshChatList();
});
$('chatSearch').addEventListener('input', (e) => onChatSearchInput(e.target.value));
$('chatSearch').addEventListener('keydown', (e) => { if (e.key === 'Escape') clearChatSearch(); });
$('showArchivedBtn').addEventListener('click', () => {
  state.sidebar.showArchived = !state.sidebar.showArchived;
  renderSidebar();
});
$('chatList').addEventListener('click', async (e) => {
  // Folder header clicks (⋯ -> Rename/Delete) are handled separately —
  // a .folder-header is not a .chatrow, so it falls outside the row
  // handling below.
  const folderActBtn = e.target.closest('[data-folder-act]');
  if (folderActBtn) {
    e.stopPropagation();
    const header = e.target.closest('.folder-header');
    const fid = Number(header.dataset.folderId);
    const act = folderActBtn.dataset.folderAct;
    if (act === 'collapse-toggle') {
      // Accordion: click the header (anything but the ⋯ menu) to show/hide
      // this folder's chats. In-memory (default collapsed after a refresh);
      // closes any open dropdown so a stray menu doesn't linger on toggle.
      if (state.sidebar.expandedFolders.has(fid)) state.sidebar.expandedFolders.delete(fid);
      else state.sidebar.expandedFolders.add(fid);
      state.sidebar.folderMenuFor = null;
      state.sidebar.moveMenuFor = null;
      state.sidebar.exportMenuFor = null;
      renderSidebar();
    } else if (act === 'toggle') {
      state.sidebar.folderMenuFor = state.sidebar.folderMenuFor === fid ? null : fid;
      state.sidebar.moveMenuFor = null;
      state.sidebar.exportMenuFor = null;
      renderSidebar();
    } else if (act === 'rename') {
      state.sidebar.folderMenuFor = null;
      const nameEl = header.querySelector('.folder-name');
      const next = window.prompt('Rename folder', nameEl ? nameEl.textContent : '');
      if (next != null && next.trim()) {
        try { await invoke('rename_folder', { id: fid, name: next.trim() }); } catch (_) {}
      }
      await refreshChatList();
    } else if (act === 'delete') {
      state.sidebar.folderMenuFor = null;
      if (!window.confirm('Delete this folder? Its chats move to Unfiled.')) { renderSidebar(); return; }
      try { await invoke('delete_folder', { id: fid }); } catch (_) {}
      await refreshChatList();
    }
    return;
  }

  const actBtn = e.target.closest('[data-act]');
  const row = e.target.closest('.chatrow');
  if (!row) return;
  const id = Number(row.dataset.id);
  if (actBtn) {
    e.stopPropagation();
    const act = actBtn.dataset.act;
    if (act === 'rename') {
      const titleEl = row.querySelector('.chatrow-title');
      const next = window.prompt('Rename chat', titleEl ? titleEl.textContent : '');
      if (next != null && next.trim()) {
        try { await invoke('rename_chat', { id, title: next.trim() }); } catch (_) {}
        await refreshChatList();
      }
    } else if (act === 'delete') {
      if (!window.confirm('Delete this conversation? This cannot be undone.')) return;
      try { await invoke('delete_chat', { id }); } catch (_) {}
      // If the deleted chat was the active one, fall back to a clean,
      // unsaved chat rather than leaving stale messages from a chat that
      // no longer exists in the store — and abort any stream still running
      // against it first (same reasoning as openChat/newChat).
      if (state.chat.chatId === id) {
        state.chat.aborter?.abort();
        state.chat.chatId = null;
        resetChatDom();
      }
      await refreshChatList();
    } else if (act === 'pin') {
      const pinned = actBtn.dataset.pinned === '1';
      try { await invoke('set_chat_pinned', { id, pinned: !pinned }); } catch (_) {}
      await refreshChatList();
    } else if (act === 'archive') {
      const archived = actBtn.dataset.archived === '1';
      state.sidebar.moveMenuFor = null;
      state.sidebar.exportMenuFor = null;
      try { await invoke('set_chat_archived', { id, archived: !archived }); } catch (_) {}
      await refreshChatList();
    } else if (act === 'move') {
      state.sidebar.moveMenuFor = state.sidebar.moveMenuFor === id ? null : id;
      state.sidebar.folderMenuFor = null;
      state.sidebar.exportMenuFor = null;
      renderSidebar();
    } else if (act === 'move-to') {
      const raw = actBtn.dataset.folderId;
      const folderId = raw === '' ? null : Number(raw);
      state.sidebar.moveMenuFor = null;
      try { await invoke('move_chat', { id, folderId }); } catch (_) {}
      await refreshChatList();
    } else if (act === 'export') {
      // §7 S7-6: a tiny submenu (Markdown/JSON/Plain text), same
      // single-open-dropdown pattern as "move to…" above.
      state.sidebar.exportMenuFor = state.sidebar.exportMenuFor === id ? null : id;
      state.sidebar.moveMenuFor = null;
      state.sidebar.folderMenuFor = null;
      renderSidebar();
    } else if (act === 'export-to') {
      const titleEl = row.querySelector('.chatrow-title');
      await exportChat(id, titleEl ? titleEl.textContent : 'chat', actBtn.dataset.format);
    }
    return;
  }
  await openChat(id);
});
// Outside-click close for the three sidebar dropdowns (move-to / export /
// folder menu) — same pattern as the profile menu below.
document.addEventListener('click', (e) => {
  if (state.sidebar.moveMenuFor == null && state.sidebar.folderMenuFor == null && state.sidebar.exportMenuFor == null) return;
  if (e.target.closest('.movemenu-wrap') || e.target.closest('.folder-menu-wrap')) return;
  state.sidebar.moveMenuFor = null;
  state.sidebar.folderMenuFor = null;
  state.sidebar.exportMenuFor = null;
  renderSidebar();
});

// §7 S7-6: export a chat to a file the user picks via the OS save sheet —
// no network, the data is already the user's (§7.5). `format` is the
// export-to button's own `data-format` ('markdown'|'json'|'txt'), passed
// straight through to `export_chat_to_file` unchanged. `dialog.save`
// returning `null` means the user cancelled the save sheet, same shape as
// `dialog.open` in `startBuild` above.
const EXPORT_EXT = { markdown: 'md', json: 'json', txt: 'txt' };
const EXPORT_FILTER_NAME = { markdown: 'Markdown', json: 'JSON', txt: 'Plain text' };
async function exportChat(id, title, format) {
  state.sidebar.exportMenuFor = null;
  renderSidebar();

  // Android: a share sheet, not a file picker. `dialog.save` is a desktop
  // assumption — on a phone "put this somewhere" means Drive, Keep, mail or
  // another chat app, and there is no filesystem path a person wants to type.
  // `share_chat` reuses the SAME formatter (`convstore::export_chat`), so the
  // bytes are identical on both platforms; only the destination differs.
  //
  // Everything below this branch is the desktop path, unchanged.
  if (IS_MOBILE) {
    try {
      await invoke('share_chat', { id, format, title: title || 'chat' });
    } catch (e) {
      showToast(String(e));
    }
    return;
  }

  const ext = EXPORT_EXT[format] || 'txt';
  const safeTitle = (title || 'chat').replace(/[\\/:*?"<>|]/g, '_').trim() || 'chat';
  let path;
  try {
    path = await window.__TAURI__.dialog.save({
      defaultPath: `${safeTitle}.${ext}`,
      filters: [{ name: EXPORT_FILTER_NAME[format] || 'File', extensions: [ext] }],
    });
  } catch (e) {
    showToast(String(e));
    return;
  }
  if (path == null) return; // cancelled
  try {
    await invoke('export_chat_to_file', { id, format, path });
    showToast('Exported to ' + path);
  } catch (e) {
    showToast(String(e));
  }
}
$('signOutBtn').addEventListener('click', async () => {
  try { await invoke('sign_out'); } catch (_) {}
  cancelPaymentPoll(null);
  closeDrawer();
  hide($('packsModal'));
  hide($('attachModal'));
  // Chat state is per-account: leave the chat view if it's open and drop
  // the transcript so the next sign-in can't replay this one's messages.
  // The conversation STORE itself is not yet account-scoped (a documented
  // v1 follow-up — see the task brief), but the sidebar must not keep
  // showing this session's chats after sign-out: clear the rendered list
  // here; refreshChatList repopulates it (for whichever account) on the
  // next enterChat.
  exitChat();
  state.chat.messages = [];
  state.chat.model = null;
  state.chat.packPaths = [];
  state.chat.chatId = null;
  $('chatList').innerHTML = '';
  // §7 S7-2b: the sidebar's folder/archive/search UI is per-account too —
  // drop it here so the next sign-in (possibly a different account) starts
  // from a clean grouped view instead of replaying this session's search
  // query or open menus over freshly-fetched data.
  state.sidebar = { chats: [], folders: [], showArchived: false, query: '', searchResults: null, moveMenuFor: null, folderMenuFor: null, exportMenuFor: null, expandedFolders: new Set() };
  clearTimeout(chatSearchDebounce);
  $('chatSearch').value = '';
  state.signedIn = false;
  state.nick = null;
  state.device = null;
  state.mine = new Set();
  state.lapsed = new Set();
  state.chatBlocked = new Set();
  resetAuthForms();
  $('device').style.display = 'none';
  closeProfileMenu();
  $('profileWrap').style.display = 'none';
  $('packsBtn').style.display = 'none';
  $('loginBtn').style.display = '';
  if (state.cat === 'mine') {
    state.cat = 'all';
    document.querySelectorAll('#nav button').forEach((x) => x.classList.toggle('active', x.dataset.cat === 'all'));
  }
  renderFilters();
  renderGrid();
});
$('billingBtn').addEventListener('click', async () => {
  const btn = $('billingBtn');
  btn.disabled = true;
  try {
    const res = await invoke('open_billing_portal');
    if (res.status === 'noBillingAccount') showToast('No billing account yet — subscribe first.');
  } catch (e) {
    showToast(String(e));
  } finally {
    btn.disabled = false;
  }
});
// The FLAG_SECURE toggle (§5.3). Deliberately does NOT close the profile menu:
// every other item here is a one-shot action, but this one has state, and
// closing the menu would hide the only feedback that the tap did anything.
$('screenPrivacyBtn').addEventListener('click', async () => {
  const next = !screenPrivacyOn();
  try { localStorage.setItem(SCREEN_PRIVACY_KEY, next ? '1' : '0'); } catch (_) {}
  await applyScreenPrivacy(next);
});
// Q1 (H6/A7): replay a synthetic slowdown so the founder can see the throttle
// notice on a COLD phone in about a second, before the ~20-minute soak starts.
//
// Like the privacy toggle this leaves the menu open, and for a sharper reason:
// the notice appears behind the menu, so closing it is the only way the result
// is visible. The label carries the outcome because a failure here MUST be
// legible — "no notice appeared" is precisely the reading Q1 exists to
// disambiguate, so an error has to say so rather than look like nothing
// happening. Same rule as the Rust side returning `Err` instead of `None`.
$('thermalSelftestBtn').addEventListener('click', async () => {
  const btn = $('thermalSelftestBtn');
  btn.disabled = true;
  btn.textContent = 'Testing…';
  try {
    const n = await invoke('chat_thermal_selftest', { run: true });
    // The label names the EDGE, because the command toggles: a second tap
    // withdraws the notice, and a button that read "Fired" both times would
    // make the two indistinguishable.
    const edge = n.throttled ? 'Raised' : 'Cleared';
    btn.textContent = `${edge} · ${n.recentTps.toFixed(1)} of ${n.baselineTps.toFixed(1)} tok/s`;
  } catch (e) {
    btn.textContent = 'FAILED — see logcat';
    console.error('thermal selftest:', e);
  }
  btn.disabled = false;
});
// A3 (spec §11): run the Stage-5 probe set through the real engine. Leaves the
// menu open for the same reason the two above it do — the label is the only
// on-screen feedback, and the real output is the logcat transcript.
//
// The label carries the SPLIT, never a score out of four: three machine
// verdicts, one that the founder still owes an answer to, and UNDECIDED as its
// own column. A button reading "4/4" would be the exact number this gate was
// built to stop producing.
$('stage5ProbeBtn').addEventListener('click', async () => {
  const btn = $('stage5ProbeBtn');
  btn.disabled = true;
  btn.textContent = 'Probing… (~1 min)';
  try {
    const v = await invoke('chat_stage5_probe', { run: true });
    const parts = [`${v.passed} pass`, `${v.failed} fail`];
    if (v.undecided) parts.push(`${v.undecided} UNDECIDED`);
    if (v.awaitingHuman) parts.push(`${v.awaitingHuman} ask-me`);
    btn.textContent = `${parts.join(' · ')} — see logcat`;
  } catch (e) {
    btn.textContent = 'FAILED — see logcat';
    console.error('stage5 probe:', e);
  }
  btn.disabled = false;
});
[['li-email', 'li-pass', 'doLogin'], ['cr-email', 'cr-pass', 'cr-nick', 'doCreate']].forEach((group) => {
  const btn = group[group.length - 1];
  group.slice(0, -1).forEach((id) => $(id).addEventListener('keydown', (e) => {
    if (e.key === 'Enter') $(btn).click();
  }));
});

boot();
