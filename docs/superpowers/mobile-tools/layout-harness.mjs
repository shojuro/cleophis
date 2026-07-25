// Headless layout-measurement harness for the Cleophis frontend.
//
// Serves the REAL src/ tree over HTTP (module imports need an origin) and
// synthesizes one extra route, /harness.html, which is src/index.html with a
// classic <script> injected immediately before the module script. Classic
// scripts run before module scripts, so the stub is installed before app.js
// evaluates `window.__TAURI__.core`.
//
// Nothing in src/ is modified. Query params drive the emulation:
//   ?ua=android      -> navigator.userAgent reports Android (transport picks mobile)
//   ?installed=1     -> download_status reports the hero installed
//   ?supported=0     -> detect_hardware reports supported:false
//   ?engine=<status> -> engine_info status (Starting|Ready|Failed)

import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const REPO = process.argv[2];
const PORT = Number(process.argv[3] || 8731);
const SRC = path.join(REPO, 'src');
const CATALOG = path.join(REPO, 'src-tauri', 'resources', 'catalog.json');

const MIME = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.mjs': 'text/javascript; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.json': 'application/json; charset=utf-8',
};

const ANDROID_UA =
  'Mozilla/5.0 (Linux; Android 13; SM-A226B) AppleWebKit/537.36 (KHTML, like Gecko) Version/4.0 Chrome/120.0.0.0 Mobile Safari/537.36';

function stub(q) {
  const catalog = JSON.parse(fs.readFileSync(CATALOG, 'utf8'));
  const forMobile = q.get('ua') === 'android';
  const installed = q.get('installed') === '1';
  const supported = q.get('supported') !== '0';
  const engineStatus = q.get('engine') || 'Ready';
  return `<script>
(function () {
  const CATALOG = ${JSON.stringify(catalog)};
  ${forMobile ? `Object.defineProperty(navigator, 'userAgent', { get: () => ${JSON.stringify(ANDROID_UA)} });` : ''}
  const listeners = {};
  window.__HARNESS__ = {
    calls: [],
    emit(name, payload) { (listeners[name] || []).forEach((f) => f({ payload })); },
  };
  const canned = {
    get_catalog: () => CATALOG.map((m) => Object.assign({}, m, { coverAbs: '/covers/' + m.id + '.webp' })),
    get_tier_selection: () => ({ mode: 'auto', activeTier: 'low', effectiveTier: 'low', committed: false, switchAvailable: true, nextChangeAt: 0 }),
    download_status: () => ({ installed: ${installed}, partBytes: 0, active: false }),
    engine_info: () => ({ port: 0, status: ${JSON.stringify(engineStatus)}, gpuOffload: false }),
    restore_session: () => ({ signedIn: false }),
    // Field names mirror the Rust struct exactly: HardwareInfo has NO serde
    // rename, so these are snake_case. A camelCase stub silently renders the
    // fallback copy and looks like a UI bug.
    detect_hardware: () => ({ tier: 'low', ram_gb: ${supported ? 8 : 3}, vram_gb: 0, platform: 'android', gpu: '', supported: ${supported} }),
    list_chats: () => [],
    list_folders: () => [],
    list_packs: () => [],
  };
  window.__TAURI__ = {
    core: {
      invoke: (cmd, args) => {
        window.__HARNESS__.calls.push([cmd, args]);
        const f = canned[cmd];
        return f ? Promise.resolve(f(args)) : Promise.reject(new Error('harness: no canned ' + cmd));
      },
      convertFileSrc: (p) => 'harness-asset:' + p,
      Channel: class { set onmessage(f) { this._f = f; } },
    },
    event: {
      listen: (name, cb) => {
        (listeners[name] = listeners[name] || []).push(cb);
        return Promise.resolve(() => {});
      },
    },
  };
})();
</script>
`;
}

const server = http.createServer((req, res) => {
  const url = new URL(req.url, 'http://localhost');
  if (url.pathname === '/harness.html') {
    const html = fs.readFileSync(path.join(SRC, 'index.html'), 'utf8');
    const marker = '<script type="module" src="app.js"></script>';
    if (!html.includes(marker)) {
      res.writeHead(500).end('harness: module script marker not found in index.html');
      return;
    }
    const out = html.replace(marker, stub(url.searchParams) + marker);
    res.writeHead(200, { 'Content-Type': MIME['.html'] }).end(out);
    return;
  }
  const rel = url.pathname === '/' ? '/index.html' : url.pathname;
  const file = path.join(SRC, path.normalize(rel).replace(/^(\.\.[/\\])+/, ''));
  if (!file.startsWith(SRC) || !fs.existsSync(file) || fs.statSync(file).isDirectory()) {
    res.writeHead(404).end('not found');
    return;
  }
  res.writeHead(200, { 'Content-Type': MIME[path.extname(file)] || 'application/octet-stream' });
  res.end(fs.readFileSync(file));
});

server.listen(PORT, '127.0.0.1', () => console.log('harness on http://127.0.0.1:' + PORT + '/harness.html'));
