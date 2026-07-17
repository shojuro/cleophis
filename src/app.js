// Cleophis front-end. Requires app.withGlobalTauri=true.
const { invoke, convertFileSrc } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const state = {
  cat: 'all', subject: 'all', q: '', signedIn: false, nick: null, device: null,
  mine: new Set(), catalog: [],
  engine: { port: 0, status: 'Starting', gpuOffload: false },
  chat: { model: null, messages: [], streaming: false, aborter: null },
  dl: { installed: false, partBytes: 0, active: false },
  pay: { modelId: null, timer: null, deadline: 0 },
  drawerId: null,
};

const $ = (id) => document.getElementById(id);

/* ---------------- boot ---------------- */
async function boot() {
  await listen('engine-ready', (e) => { state.engine = e.payload; hideEngineBanner(); setComposerEnabled(true); });
  await listen('engine-restarting', () => { showEngineBanner('Local engine restarting…'); setComposerEnabled(false); });
  await listen('engine-failed', (e) => { showEngineBanner('Local engine failed: ' + e.payload); setComposerEnabled(false); });
  await listen('download-progress', onDownloadProgress);
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
    `<button class="chip ${state.subject === s ? 'active' : ''}" data-subject="${s}">${s === 'all' ? 'All subjects' : s}</button>`
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
    return `<button class="card" data-id="${m.id}">
      <div class="cover"><img src="${m.coverUrl}" alt="" loading="lazy"/>
        <span class="tag ${m.category}">${m.category === 'education' ? 'Education' : 'Medical'}</span></div>
      <div class="cardbody">
        <h3>${m.name}</h3>
        <p class="sub">${m.subject}</p>
        <div class="cardfoot">
          <span class="compat ${cp.cls}">${cp.label}</span>
          <span class="meta mono">${m.sizeParams} · ${m.pro ? 'Pro' : m.price}</span>
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
  const gb = (m.fileBytes / 2 ** 30).toFixed(2);
  const check = '<svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><path d="M20 6L9 17l-5-5"/></svg>';
  const gib = (m.fileBytes / 2 ** 30).toFixed(2);
  const btnLabel = m.real
    ? (state.dl.installed
        ? 'Open chat'
        : state.dl.active
          ? 'Downloading…'
          : state.dl.partBytes > 0
            ? `Resume download · ${(state.dl.partBytes / 2 ** 30).toFixed(2)} of ${gib} GiB`
            : state.pay.modelId === m.id
              ? 'Waiting for payment… (click to cancel)'
              : state.mine.has(m.id)
                ? `Download · ${gib} GiB`
                : `Get · ${m.pro ? 'Pro' : m.price}`)
    : (installed ? 'Installed' : `Download · ${m.pro ? 'Pro' : m.price}`);
  $('drawer').innerHTML = `
    <button class="x" data-close>&times;</button>
    <div class="dcover"><img src="${m.coverUrl}" alt=""/></div>
    <span class="tag ${m.category}" style="position:static;display:inline-block;margin-top:14px">${m.category === 'education' ? 'Education' : 'Medical reference'}</span>
    <h2>${m.name}</h2>
    <div class="dsub">${m.subject} · ${gb} GiB on disk</div>
    <div class="specs">
      <div class="spec"><div class="k">Model size</div><div class="v mono">${m.sizeParams} params</div></div>
      <div class="spec"><div class="k">On your device</div><div class="v"><span class="compat ${cp.cls}">${cp.label}</span></div></div>
      <div class="spec"><div class="k">Speed</div><div class="v mono">${m.tps || '—'}</div></div>
      <div class="spec"><div class="k">Eval</div><div class="v">${m.eval || '—'}</div></div>
    </div>
    <div class="dlrow">
      <button class="btn primary block" id="dlBtn">${btnLabel}</button>
      <div class="prog" id="prog"><i></i></div>
      <div class="dlline mono" id="dlLine" style="display:none;font-size:12.5px;color:var(--muted);margin-top:8px"></div>
      <div class="installed" id="installedMsg">${check} Installed — runs offline on your device</div>
      <div class="errmsg" id="errMsg"></div>
    </div>
    <div class="body">
      <h4>About</h4><p>${m.long || m.blurb}</p>
      <h4>What's inside</h4>
      <ul class="inside">${(m.inside || []).map((i) => `<li>${check}${i}</li>`).join('')}</ul>
    </div>`;
  $('scrim').classList.add('show'); $('drawer').classList.add('show');
  $('drawer').setAttribute('aria-hidden', 'false');
  if (installed && !m.real) $('installedMsg').style.display = 'flex';
  $('dlBtn').onclick = () => runGetFlow(m, $('dlBtn'));
}

function closeDrawer() {
  $('scrim').classList.remove('show'); $('drawer').classList.remove('show');
  $('drawer').setAttribute('aria-hidden', 'true');
  state.drawerId = null;
}

/* ---------------- Get flow ---------------- */
function runGetFlow(m, btn) {
  if (m.real) {
    if (state.dl.installed) { enterChat(m); return; }
    if (state.dl.active) return;
    const startDownload = () => {
      state.mine.add(m.id);
      renderGrid();
      heroDownload(m, btn);
    };
    if (state.mine.has(m.id) || state.dl.partBytes > 0) { startDownload(); return; }
    if (state.pay.modelId === m.id) {
      cancelPaymentPoll(`Get · ${m.pro ? 'Pro' : m.price}`);
      return;
    }
    btn.disabled = true;
    btn.textContent = 'Opening checkout…';
    (async () => {
      let res;
      try {
        res = await invoke('start_checkout', { modelId: m.id });
      } catch (e) {
        btn.disabled = false;
        btn.textContent = `Get · ${m.pro ? 'Pro' : m.price}`;
        const el = $('errMsg'); el.style.display = 'block'; el.style.color = ''; el.textContent = String(e);
        return;
      }
      if (res.status === 'alreadyOwned') {
        state.mine.add(m.id);
        renderGrid();
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
      beginPaymentPoll(m);
    })();
  } else {
    if (state.mine.has(m.id)) return;
    simulateStubDownload(m, btn);
  }
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

function cancelPaymentPoll(resetBtnText) {
  if (state.pay.timer) clearInterval(state.pay.timer);
  state.pay = { modelId: null, timer: null, deadline: 0 };
  const btn = $('dlBtn');
  if (btn && resetBtnText) { btn.disabled = false; btn.textContent = resetBtnText; }
  if (resetBtnText) {
    const el = $('errMsg');
    if (el) { el.style.display = 'none'; el.style.color = ''; }
  }
}

function beginPaymentPoll(m) {
  state.pay.modelId = m.id;
  state.pay.deadline = Date.now() + 10 * 60 * 1000;
  state.pay.timer = setInterval(async () => {
    if (Date.now() > state.pay.deadline) {
      const drawerMatch = state.drawerId === m.id;
      cancelPaymentPoll(drawerMatch ? `Get · ${m.pro ? 'Pro' : m.price}` : null);
      if (drawerMatch) {
        const el = $('errMsg');
        if (el) { el.style.display = 'block'; el.style.color = ''; el.textContent = "We didn't see a completed payment. If you paid, it will appear shortly — try Get again in a moment."; }
      }
      return;
    }
    let list;
    try { list = await invoke('list_entitlements'); } catch (_) { return; }
    const hit = (list || []).some((e) => e.modelId === m.id && e.source === 'purchase');
    if (!hit) return;
    cancelPaymentPoll(null);
    state.mine.add(m.id);
    renderGrid();
    const btn = $('dlBtn');
    if (btn && state.drawerId === m.id && $('drawer').classList.contains('show')) {
      btn.disabled = true;
      btn.textContent = '✓ Paid — starting download…';
      heroDownload(m, btn);
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

/* ---------------- chat ---------------- */
function enterChat(m) {
  state.chat.model = m;
  $('chatModelName').textContent = m.name;
  $('chatCover').src = m.coverUrl;
  rebuildChatDom();
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
  for (const msg of state.chat.messages) appendBubble(msg.role, msg.content);
}

function appendBubble(role, text) {
  const el = document.createElement('div');
  el.className = `msg ${role}`;
  el.textContent = text;
  $('chatMessages').appendChild(el);
  $('chatMessages').scrollTop = $('chatMessages').scrollHeight;
  return el;
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
  sendCompletion();
}

async function sendCompletion() {
  const m = state.chat.model;
  document.querySelectorAll('.retrychip').forEach((el) => el.remove());
  state.chat.streaming = true;
  $('sendBtn').hidden = true; $('stopBtn').hidden = false;
  const bubble = appendBubble('assistant', '');
  bubble.classList.add('streaming');
  let acc = '';
  state.chat.aborter = new AbortController();
  try {
    const res = await fetch(`http://127.0.0.1:${state.engine.port}/v1/chat/completions`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      signal: state.chat.aborter.signal,
      body: JSON.stringify({
        messages: [
          { role: 'system', content: m.systemPrompt },
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
        if (data === '[DONE]') { finishStream(bubble, acc); return; }
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
    finishStream(bubble, acc);
  } catch (err) {
    if (err.name === 'AbortError') { finishStream(bubble, acc); return; }
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

function finishStream(bubble, acc) {
  bubble.classList.remove('streaming');
  if (acc) state.chat.messages.push({ role: 'assistant', content: acc });
  else bubble.remove();
  state.chat.streaming = false;
  $('sendBtn').hidden = false; $('stopBtn').hidden = true;
  $('sendBtn').disabled = false;
  pulseCost();
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
function hide(el) { el.classList.remove('show'); }

async function applySession(info) {
  state.signedIn = true;
  state.nick = info.nickname || 'you';
  state.mine = new Set((info.entitlements || []).map((e) => e.modelId));
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
  hide($('loginModal'));
  hide($('createModal'));
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
  if (e.key === 'Escape') { closeDrawer(); hide($('loginModal')); hide($('createModal')); }
});
$('loginBtn').addEventListener('click', () => {
  if (state.signedIn) {
    state.cat = 'mine';
    document.querySelectorAll('#nav button').forEach((x) => x.classList.toggle('active', x.dataset.cat === 'mine'));
    renderFilters(); renderGrid();
  } else show($('loginModal'));
});
document.querySelectorAll('[data-close]').forEach((b) => b.addEventListener('click', () => { hide($('loginModal')); hide($('createModal')); }));
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
    await applySession(await invoke('sign_in', { email, password }));
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
    await applySession(await invoke('sign_up', { email, password, nickname: $('cr-nick').value.trim() || 'you' }));
  } catch (e) {
    err.style.display = 'block';
    err.textContent = String(e);
  } finally {
    btn.disabled = false;
    btn.textContent = 'Create account';
  }
});
[$('loginModal'), $('createModal')].forEach((md) => md.addEventListener('click', (e) => { if (e.target === md) hide(md); }));
$('chatBack').addEventListener('click', () => exitChat());
$('sendBtn').addEventListener('click', () => sendMessage());
$('chatInput').addEventListener('keydown', (e) => { if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); sendMessage(); } });
$('stopBtn').addEventListener('click', () => state.chat.aborter?.abort());
$('signOutBtn').addEventListener('click', async () => {
  try { await invoke('sign_out'); } catch (_) {}
  cancelPaymentPoll(null);
  state.signedIn = false;
  state.nick = null;
  state.device = null;
  state.mine = new Set();
  $('device').style.display = 'none';
  $('signOutBtn').style.display = 'none';
  $('billingBtn').style.display = 'none';
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
