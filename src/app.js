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
async function openPacksModal() {
  show($('packsModal'));
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
    if (buildBtn) buildBtn.disabled = false;
    if (nameInput) { nameInput.disabled = false; nameInput.value = ''; }
    await refreshPacksList();
  } catch (e) {
    if (err) { err.style.display = 'block'; err.textContent = String(e); }
    if (prog) prog.style.display = 'none';
    if (cancelBtn) cancelBtn.style.display = 'none';
    if (buildBtn) buildBtn.disabled = false;
    if (nameInput) nameInput.disabled = false;
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
  const list = $('chatList');
  let chats;
  try {
    chats = await invoke('list_chats');
  } catch (_) {
    return; // additive UI — leave whatever's already rendered on failure
  }
  if (!chats.length) {
    list.innerHTML = `<div class="chatlist-empty">No conversations yet.</div>`;
    return;
  }
  // title is user-renameable (untrusted) — escapeHtml it; pin/rename/delete
  // are static labels, not interpolated user data.
  list.innerHTML = chats.map((c) => `
    <div class="chatrow ${c.id === state.chat.chatId ? 'active' : ''}" data-id="${c.id}">
      <span class="chatrow-title">${escapeHtml(c.title)}</span>
      <span class="chatrow-actions">
        <button class="chatrow-act" data-act="pin" data-pinned="${c.pinned ? '1' : '0'}" title="${c.pinned ? 'Unpin' : 'Pin'}">${c.pinned ? '★' : '☆'}</button>
        <button class="chatrow-act" data-act="rename" title="Rename">✎</button>
        <button class="chatrow-act" data-act="delete" title="Delete">✕</button>
      </span>
    </div>`).join('');
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
    row.textContent = `[${c.n}] ${c.docTitle}` +
      (c.sectionPath ? ` · ${c.sectionPath}` : '') +
      (c.locator ? ` · ${c.locator}` : '');
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
    const res = await fetch(`http://127.0.0.1:${state.engine.port}/v1/chat/completions`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      signal: state.chat.aborter.signal,
      body: JSON.stringify({
        messages: [
          { role: 'system', content: groundedPrompt ?? m.systemPrompt },
          { role: 'assistant', content: m.greeting },
          ...state.chat.messages,
        ],
        stream: true,
        max_tokens: 512,
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
        if (data === '[DONE]') { finishStream(bubble, acc, groundedCitations, turnChatId); return; }
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
    finishStream(bubble, acc, groundedCitations, turnChatId);
  } catch (err) {
    if (err.name === 'AbortError') { finishStream(bubble, acc, groundedCitations, turnChatId); return; }
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
function finishStream(bubble, acc, citations, turnChatId) {
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
    const err = $('packErr');
    err.style.display = 'none';
    err.textContent = '';
  } else if (el.id === 'attachModal') {
    $('attachList').innerHTML = '';
  }
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
  const lb = $('loginBtn');
  lb.textContent = state.nick;
  lb.title = info.mode === 'offlineCached' ? 'Signed in — offline, using saved account data' : '';
  $('signOutBtn').style.display = '';
  $('billingBtn').style.display = '';
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
  if (e.key === 'Escape') { closeDrawer(); hide($('loginModal')); hide($('createModal')); hide($('packsModal')); hide($('attachModal')); }
});
$('loginBtn').addEventListener('click', () => {
  if (state.signedIn) {
    state.cat = 'mine';
    document.querySelectorAll('#nav button').forEach((x) => x.classList.toggle('active', x.dataset.cat === 'mine'));
    renderFilters(); renderGrid();
  } else show($('loginModal'));
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
$('chatList').addEventListener('click', async (e) => {
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
    }
    return;
  }
  await openChat(id);
});
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
  state.signedIn = false;
  state.nick = null;
  state.device = null;
  state.mine = new Set();
  state.lapsed = new Set();
  state.chatBlocked = new Set();
  resetAuthForms();
  $('device').style.display = 'none';
  $('signOutBtn').style.display = 'none';
  $('billingBtn').style.display = 'none';
  $('packsBtn').style.display = 'none';
  const lb = $('loginBtn');
  lb.textContent = 'Log in';
  lb.title = '';
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
