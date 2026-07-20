// Cleophis front-end. Requires app.withGlobalTauri=true.
const { invoke, convertFileSrc } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

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
  pay: { modelId: null, timer: null, deadline: 0, btnId: null },
  drawerId: null,
  // §7 S7-2b: sidebar organization — folders/chats cache backing the
  // grouped render, plus the search view and the three single-open
  // dropdowns (a chat's "Move to…" list, a chat's export-format menu
  // [§7 S7-6], a folder's Rename/Delete menu).
  sidebar: { chats: [], folders: [], showArchived: false, query: '', searchResults: null, moveMenuFor: null, folderMenuFor: null, exportMenuFor: null, expandedFolders: new Set() },
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

/* ---------------- boot ---------------- */
async function boot() {
  await listen('engine-ready', (e) => { state.engine = e.payload; hideEngineBanner(); setComposerEnabled(true); });
  await listen('engine-restarting', () => { showEngineBanner('Local engine restarting…'); setComposerEnabled(false); });
  await listen('engine-failed', (e) => { showEngineBanner('Local engine failed: ' + e.payload); setComposerEnabled(false); });
  await listen('download-progress', onDownloadProgress);
  await listen('build-progress', onBuildProgress);
  state.catalog = await invoke('get_catalog');
  for (const m of state.catalog) m.coverUrl = convertFileSrc(m.coverAbs);
  const heroEntry = state.catalog.find((m) => m.real);
  if (heroEntry) {
    try {
      const ds = await invoke('download_status', { modelId: heroEntry.id });
      state.dl = { installed: ds.installed, partBytes: ds.partBytes, active: ds.active };
    } catch (_) {}
  }
  try { state.engine = await invoke('engine_info'); } catch (_) {}
  renderFilters(); renderGrid();
  try {
    const s = await invoke('restore_session');
    if (s.signedIn) await applySession(s);
  } catch (_) { /* signed-out boot is fine */ }
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

/* ---------------- drawer ---------------- */
function openDrawer(id) {
  const m = state.catalog.find((x) => x.id === id); if (!m) return;
  state.drawerId = m.id;
  const cp = compat(m.sizeParams);
  const installed = state.mine.has(m.id);
  const lapsed = m.real && state.lapsed.has(m.id);
  const chatBlocked = m.real && state.chatBlocked.has(m.id);
  const gb = (m.fileBytes / 2 ** 30).toFixed(2);
  const check = '<svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><path d="M20 6L9 17l-5-5"/></svg>';
  const gib = (m.fileBytes / 2 ** 30).toFixed(2);
  const waitingLabel = 'Waiting for payment… (click to cancel)';
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
      <button class="btn primary block" id="dlBtn">${btnLabel}</button>
      <div class="prog" id="prog"><i></i></div>
      <div class="dlline mono" id="dlLine" style="display:none;font-size:12.5px;color:var(--muted);margin-top:8px"></div>
      <div class="installed" id="installedMsg">${check} Installed — runs offline on your device</div>
      <div class="errmsg" id="errMsg"></div>
      ${renewLineHtml}
    </div>
    <div class="body">
      <h4>About</h4><p>${escapeHtml(m.long || m.blurb)}</p>
      <h4>What's inside</h4>
      <ul class="inside">${(m.inside || []).map((i) => `<li>${check}${escapeHtml(i)}</li>`).join('')}</ul>
    </div>`;
  $('scrim').classList.add('show'); $('drawer').classList.add('show');
  $('drawer').setAttribute('aria-hidden', 'false');
  if (installed && !m.real) $('installedMsg').style.display = 'flex';
  $('dlBtn').onclick = () => runGetFlow(m, $('dlBtn'));
  if (showRenewLine) {
    const renewEl = $('renewLine');
    renewEl.onclick = () => startCheckoutFlow(m, renewEl);
  }
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

function heroDownload(m, btn) {
  const prog = $('prog'), bar = prog.firstElementChild;
  prog.style.display = 'block';
  btn.disabled = true;
  btn.textContent = 'Downloading…';
  state.dl.active = true;
  invoke('download_model', { modelId: m.id }).catch((err) => {
    state.dl.active = false;
    prog.style.display = 'none';
    btn.disabled = false;
    btn.textContent = 'Retry download';
    const el = $('errMsg'); el.style.display = 'block'; el.style.color = ''; el.textContent = String(err);
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

async function onDownloadProgress(e) {
  const p = e.payload;
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
  if (p.phase === 'parsing') {
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
    citations: msg.citations && msg.citations.length ? msg.citations : undefined,
  }));
  rebuildChatDom();
  updateGroundPill();
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
      adapterIds: [],
    });
    chatId = chat.id;
  } catch (_) { /* persistence failed — still hand back a clean local chat */ }
  state.chat.chatId = chatId;
  resetChatDom();
  await refreshChatList();
}

/* ---------------- chat ---------------- */

// §7 S7-4: fit each request to n_ctx. Mirror the engine: server runs with
// `-c 4096` (inference.rs) and each request reserves max_tokens for the
// reply. We keep the most-recent messages that fit the remaining budget so
// a long chat never overflows n_ctx (which would make llama.cpp
// context-shift/truncate unpredictably — silent quality loss or errors).
const N_CTX = 4096;         // must match inference.rs `-c`
const REPLY_RESERVE = 512;  // must match the request's max_tokens
const CTX_SAFETY = 128;     // headroom for tokenizer estimate error + framing
// Appended to the system prompt on UNGROUNDED turns (no packs attached this
// turn). Without grounding the base model will otherwise parrot/fabricate
// "source titles" from earlier grounded turns still in the transcript — the
// grounded path is hardened symmetrically in retrieve.rs. Interim mitigation;
// the contract-trained LoRA adapter is the real fix for grounding-honesty.
const UNGROUNDED_NO_SOURCES_NOTE = ' No documents are attached to this conversation, so you have no sources to cite. Do not list, cite, or invent source titles; if asked about your sources, say none are attached.';
// Deliberately conservative (~3.5 chars/token OVER-estimates tokens → we
// under-fill and stay under n_ctx rather than risk overflow).
function estTokens(s) { return Math.ceil((s ? s.length : 0) / 3.5) + 4; /* +4 ≈ role framing */ }

// Returns the most-recent contiguous suffix of `messages` that fits the
// budget left after the fixed preamble (system + greeting) and the reply
// reserve, plus how many older messages were dropped. Always keeps at
// least the final message (the current user turn) even if it alone is huge
// (degenerate — the engine will truncate that one; extremely rare). Pure —
// no state reads — so it's trivially reasoned-about and testable.
function windowMessages(messages, systemContent, greetingContent) {
  const budget = N_CTX - REPLY_RESERVE - CTX_SAFETY
    - estTokens(systemContent) - estTokens(greetingContent);
  let used = 0, startIdx = messages.length;
  for (let i = messages.length - 1; i >= 0; i--) {
    const t = estTokens(messages[i].content);
    if (i < messages.length - 1 && used + t > budget) break; // always keep the last
    used += t; startIdx = i;
  }
  return { sent: messages.slice(startIdx), droppedCount: startIdx };
}

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
  for (const msg of state.chat.messages) appendBubble(msg.role, msg.content, msg.citations);
  updateContextDivider();
}

function appendBubble(role, text, citations) {
  const el = document.createElement('div');
  el.className = `msg ${role}`;
  el.textContent = text;
  $('chatMessages').appendChild(el);
  if (citations && citations.length) renderCitations(el, citations);
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
  const { droppedCount } = windowMessages(state.chat.messages, m.systemPrompt, m.greeting);
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

function setComposerEnabled(on) {
  $('chatInput').disabled = !on;
  $('sendBtn').disabled = !on || state.chat.streaming;
}

function showEngineBanner(text) { const b = $('engineBanner'); b.hidden = false; b.textContent = text; }
function hideEngineBanner() { $('engineBanner').hidden = true; }

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
  document.querySelectorAll('.retrychip').forEach((el) => el.remove());
  const bubble = appendBubble('assistant', '');
  bubble.classList.add('streaming');
  let acc = '';

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
          adapterIds: [],
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
  let groundedPrompt = null, groundedCitations = null;
  if (state.chat.packPaths.length > 0) {
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
        if (state.chat.aborter.signal.aborted) { finishStream(bubble, '', null, turnChatId); return; }
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
      if (state.chat.aborter.signal.aborted) { finishStream(bubble, '', null, turnChatId); return; }
      if (rag.status === 'noEvidence') {
        // Short-circuit to a deterministic refusal rather than handing the
        // model rag.prompt's [[NO_EVIDENCE]] marker: without the
        // contract-trained adapter (parked track — see the §3a plan) the
        // base model won't reliably refuse on its own, so a scripted
        // refusal is the honest, demo-safe interim. Once the adapter
        // lands, this branch can feed the marker to the model instead.
        const n = state.chat.packPaths.length;
        const refusal = `I couldn't find anything about that in your attached pack${n > 1 ? 's' : ''}, so I won't guess. Try rephrasing, or attach a pack that covers it.`;
        // §7 S7-2: only touch the live DOM/in-memory transcript if this
        // turn's chat is STILL the active one (mirrors finishStream below)
        // — the user may have switched away while rag_query was in flight.
        const isActive = turnChatId === state.chat.chatId;
        bubble.classList.remove('streaming');
        if (isActive) {
          bubble.textContent = refusal;
          state.chat.messages.push({ role: 'assistant', content: refusal, noEvidence: true });
        }
        // Persist to the chat this turn actually belongs to, fire-and-
        // forget (never blocks the composer restore below).
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
        // §7 S7-5: this branch returns without calling finishStream, so a
        // no-evidence first turn never triggers auto-titling — the
        // first-line title stands. Acceptable v1: a refusal has nothing
        // worth summarizing into a title anyway.
        return; // no model call — pulseCost() intentionally skipped, no inference ran
      }
      // grounded: swap this turn's system message for the assembled
      // grounded prompt (contract + numbered sources) and continue into
      // the normal streaming path below.
      groundedPrompt = rag.prompt;
      groundedCitations = rag.citations;
      bubble.textContent = '';
      const pill = $('groundPill');
      pill.hidden = false;
      pill.textContent = `Grounded in ${groundedCitations.length} source${groundedCitations.length === 1 ? '' : 's'}`;
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
    const sys = groundedPrompt != null ? groundedPrompt : m.systemPrompt + UNGROUNDED_NO_SOURCES_NOTE;
    const win = windowMessages(state.chat.messages, sys, m.greeting);
    const res = await fetch(`http://127.0.0.1:${state.engine.port}/v1/chat/completions`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      signal: state.chat.aborter.signal,
      body: JSON.stringify({
        messages: [
          { role: 'system', content: sys },
          { role: 'assistant', content: m.greeting },
          ...win.sent,
        ],
        stream: true,
        max_tokens: REPLY_RESERVE, // bound to the windowing reserve so the two can't drift
        temperature: 0.7,
        cache_prompt: true,
      }),
    });
    if (!res.ok) throw new Error(`engine returned ${res.status}`);
    const reader = res.body.getReader();
    const dec = new TextDecoder();
    let buf = '';
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      buf += dec.decode(value, { stream: true });
      let nl;
      while ((nl = buf.indexOf('\n')) >= 0) {
        const line = buf.slice(0, nl).trim();
        buf = buf.slice(nl + 1);
        if (!line.startsWith('data: ')) continue;
        const data = line.slice(6);
        if (data === '[DONE]') { finishStream(bubble, acc, groundedCitations, turnChatId, autoTitle); return; }
        try {
          const delta = JSON.parse(data).choices?.[0]?.delta?.content;
          if (delta) {
            acc += delta;
            bubble.textContent = acc;
            $('chatMessages').scrollTop = $('chatMessages').scrollHeight;
          }
        } catch (_) { /* partial line — ignored */ }
      }
    }
    finishStream(bubble, acc, groundedCitations, turnChatId, autoTitle);
  } catch (err) {
    if (err.name === 'AbortError') { finishStream(bubble, acc, groundedCitations, turnChatId, autoTitle); return; }
    bubble.remove();
    state.chat.streaming = false;
    $('sendBtn').hidden = false; $('stopBtn').hidden = true;
    try {
      const info = await invoke('engine_info');
      state.engine = info;
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
function finishStream(bubble, acc, citations, turnChatId, autoTitle) {
  bubble.classList.remove('streaming');
  // Only touch the live DOM/in-memory transcript if this turn's chat is
  // STILL the active one — the user may have switched chats mid-stream
  // (which aborts this turn's fetch, via openChat/newChat, but whatever
  // partial `acc` had already accumulated still gets here). A turn whose
  // chat is no longer active must not repaint over whatever chat is on
  // screen now; it still gets PERSISTED to its own chat below.
  const isActive = turnChatId === state.chat.chatId;
  if (isActive) {
    if (acc) {
      const msg = { role: 'assistant', content: acc };
      // Stash citations on the pushed message (not just rendered here) so
      // rebuildChatDom can replay them if the chat is exited and re-entered.
      if (citations && citations.length) {
        msg.citations = citations;
        renderCitations(bubble, citations);
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
  if (acc && turnChatId != null) {
    invoke('append_message', {
      chatId: turnChatId,
      role: 'assistant',
      content: acc,
      citations: citations && citations.length ? citations : null,
    }).then(refreshChatList).catch(() => {}); // updated_at bump reorders the sidebar
  }
  // §7 S7-5: fire-and-forget the auto-title generation for this turn — do
  // NOT await it (it must never gate the composer restore above, which
  // already ran). turnChatId-scoped like the persist above, so a
  // mid-stream chat switch still titles the right chat.
  if (acc && turnChatId != null && autoTitle) maybeAutoTitle(turnChatId, autoTitle.source, acc);
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
    let res;
    try {
      res = await fetch(`http://127.0.0.1:${state.engine.port}/v1/chat/completions`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        signal: aborter.signal,
        body: JSON.stringify({
          messages: [
            { role: 'system', content: 'You write very short chat titles. Reply with ONLY a 3–6 word title for the conversation. No quotes, no trailing punctuation, no preamble, no "Title:".' },
            { role: 'user', content: `User: ${userText.slice(0, 500)}\nAssistant: ${assistantText.slice(0, 500)}\n\nTitle:` },
          ],
          stream: false,
          max_tokens: 24,
          temperature: 0.3,
          cache_prompt: false,
        }),
      });
    } finally {
      clearTimeout(timer);
    }
    if (!res.ok) return;
    const data = await res.json();
    const raw = data.choices?.[0]?.message?.content;
    if (!raw) return;
    // Sanitize: first line only, strip an echoed "Title:" prefix, strip
    // surrounding quotes/backticks/asterisks, collapse whitespace, trim,
    // strip trailing punctuation, cap length.
    let title = raw.split('\n')[0];
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
    closeDrawer(); hide($('loginModal')); hide($('createModal')); hide($('packsModal')); hide($('attachModal')); closeProfileMenu();
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
document.querySelectorAll('[data-close]').forEach((b) => b.addEventListener('click', () => { hide($('loginModal')); hide($('createModal')); hide($('packsModal')); hide($('attachModal')); }));
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
  if (password.length < 8) {
    err.style.display = 'block';
    err.textContent = 'Password must be at least 8 characters.';
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
[$('loginModal'), $('createModal'), $('packsModal'), $('attachModal')].forEach((md) => md.addEventListener('click', (e) => { if (e.target === md) hide(md); }));
$('chatBack').addEventListener('click', () => exitChat());
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
[['li-email', 'li-pass', 'doLogin'], ['cr-email', 'cr-pass', 'cr-nick', 'doCreate']].forEach((group) => {
  const btn = group[group.length - 1];
  group.slice(0, -1).forEach((id) => $(id).addEventListener('keydown', (e) => {
    if (e.key === 'Enter') $(btn).click();
  }));
});

boot();
