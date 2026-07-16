# Cleophis Desktop — Demo Build Design

**Date:** 2026-07-16
**Status:** Approved section-by-section in brainstorming; pending final user review
**Source spec:** "Cleophis Desktop — Build Spec (Demo Scope)" (investor-demo build)

## 1. Objective

A Windows desktop app that is the store, the model manager, and the chat window in one. A non-technical user browses a visual catalog, clicks **Get** on a model, and slides directly into a live chat with that model running locally — offline, streaming, $0.00. This build exists to be demoed to investors on one specific laptop.

## 2. Decisions made (with the user, 2026-07-16)

| Question | Decision |
|---|---|
| Demo platform | **Windows first.** The demo runs on this exact machine: Windows 11 + WSL2, 16 GB RAM, 12 threads, NVIDIA GTX 1650 (4 GB VRAM). macOS build deferred entirely. |
| Storefront prototype | Exists at `C:\Users\JM505 Computers\Downloads\storefront.html` ("Josiah's Library", 428-line single-file prototype). Reuse and adapt it. |
| Hero model | Off-the-shelf ~3B instruct GGUF; Socratic behavior carried by the system prompt. No fine-tune for the demo. |
| Timeline | Demo is 2–4 weeks out. |
| Branding | **Cleophis** everywhere user-visible (window title, header brand, installer, identifier). Ink+teal visual identity carried over from the prototype. |
| Cover art | Programmatically generated placeholder art (owned, clean-IP), swappable later via the `covers/` folder. |
| Engine integration | **Spec Option B: llama-server sidecar** (prebuilt official binary). Option A (in-process `llama-cpp-2`) is the post-demo refactor path. |

## 3. Architecture

**Repo:** `C:\Users\JM505 Computers\dev\cleophis` — on the Windows filesystem because the Windows-native toolchain (rustup/cargo + VS 2022 MSVC + Node 20, all present or one install away) must build it; Windows tools cannot build from a WSL UNC path. Editing happens from WSL via `/mnt/c`; builds are driven through WSL→Windows interop (`cmd.exe`/`pwsh.exe`).

**Two processes at runtime:**

1. **`Cleophis.exe`** — Tauri 2 shell (system WebView2). The Rust core does hardware detection, catalog resolution, and sidecar lifecycle. The UI is the adapted storefront: vanilla JS, single page, two views.
2. **`llama-server.exe`** — official prebuilt llama.cpp release binary, **Vulkan build** (whole 3B Q4 fits in the 1650's 4 GB VRAM; no CUDA-runtime bloat), bundled as a Tauri sidecar. Spawned at app launch on `127.0.0.1:<free port>` with the bundled GGUF. The model is **pre-warmed during catalog browsing**, which is what makes the zero-dead-time download→chat transition honest.

**Streaming path:** front-end JS calls `POST http://127.0.0.1:<port>/v1/chat/completions` with `stream: true` and renders SSE deltas token-by-token. No Rust re-emission layer. Tauri CSP allows connections to `http://127.0.0.1:*` only.

**Layout** (mirrors source spec §2):

```
cleophis/
├─ src-tauri/
│  ├─ src/main.rs          # app entry, window config
│  ├─ src/inference.rs     # sidecar lifecycle: spawn, health, watchdog, kill
│  ├─ src/hardware.rs      # RAM/GPU detection → device tier
│  ├─ src/catalog.rs       # reads bundled catalog.json, resolves model paths
│  ├─ resources/
│  │  ├─ models/<hero>.gguf
│  │  ├─ covers/*.webp     # 11 generated covers
│  │  └─ catalog.json
│  ├─ binaries/            # llama-server sidecar (Windows Vulkan build)
│  └─ tauri.conf.json
├─ src/                    # index.html, app.js, styles.css (adapted storefront)
├─ tools/covers/           # Node script that generates cover art
└─ docs/superpowers/specs/ # this document
```

## 4. Front-end: views & the transition

**View 1 — Catalog.** Prototype structure survives: sticky header, category nav (All/Education/Medical/My library), subject chips, search, detail drawer, mock sign-in (modal → toggles device chip and per-card compatibility badges). Changes:
- Brand becomes **Cleophis**.
- Cards become **cover-forward**: generated cover fills the top ~70% of the card; name/subject small beneath; compat badge + size in a slim footer. This is the deliberate anti–Hugging-Face choice.
- The prototype's 11 hardcoded entries move to `catalog.json` (loaded via Tauri resource). Pricing chips stay; nothing gates on them.

**The download→chat transition (the emotional payoff — build precisely):**
1. User clicks **Get** in the drawer → progress bar fills over ~2.5 s (theater; the model is bundled and already warm).
2. Completion awaits the `engine-ready` state (normally already true), then the catalog+drawer **slide left as one plane and the chat view slides in from the right**.
3. The tutor's `greeting` is already printed in the thread and focus is already in the input box. **No "Installed · open" intermediate step for the hero model.**
4. Stub models (`real: false`) keep the prototype's old behavior: simulated download → "Installed" in My Library, no chat. Only the hero enters chat; the demo script only clicks the hero.

**View 2 — Chat.**
- Message list; assistant tokens render delta-by-delta with a live caret.
- Input + send (Enter submits). A **Stop** button aborts the SSE stream (runaway generation mid-pitch is a real risk).
- Header: model name · **"● Local · offline"** pill · **$0.00 cost counter** pinned right, re-rendered after every completed message, never changing.
- Back arrow → reverse slide to catalog. Sidecar keeps the model loaded; chat history preserved for the session; re-entry instant.

## 5. Rust core, sidecar lifecycle & error handling

**Tauri commands (thin on purpose — chat goes straight to the sidecar over HTTP):**
- `detect_hardware() -> {ram_gb, gpu, platform, tier}` — RAM via `sysinfo`; GPU name via WMI (`Win32_VideoController`). Tier map per source spec §6. This machine lands on **mid**; the prototype's compat logic shows "Runs great" for ≤3B on mid, so the hero badge reads right on demo day.
- `engine_info() -> {port, status}` — plus an `engine-ready` event emitted when health checks pass.
- `load_model(model_id) -> Result<()>` — idempotent "ensure ready" gate for the single pre-warmed model; kept as the seam for the post-demo multi-model world. The Get button's completion awaits it.
- `unload_model()` — **not implemented** for the demo (single model, 16 GB machine).

**Sidecar lifecycle:**
- Startup: pick a free port (bind `:0`, read, release; retry ×3), spawn sidecar: `llama-server --host 127.0.0.1 --port <N> -m <resource gguf> -ngl 99 -c 4096 --no-webui`, poll `GET /health` until OK → emit `engine-ready`.
- **Fallback chain:** Vulkan init failure or early exit → respawn with `-ngl 0` (pure CPU; a 3B Q4 still streams acceptably on 12 threads). Death mid-chat → watchdog respawns, UI shows "reconnecting", input re-enables on health.
- App exit kills the child process.

**Prompt assembly (client-side, from `catalog.json`):** messages = `[system: systemPrompt, assistant: greeting, ...conversation]` so the model continues in voice. Generation params: `max_tokens ≈ 512`, `temperature ≈ 0.7` — snappy demo answers.

**Error surfaces:** failed/aborted stream → message marked with a retry affordance, never a raw error dump; Get before engine-ready → progress bar waits invisibly.

## 6. Catalog data, covers & hero model

**`catalog.json`** uses the source spec §3 schema exactly. The prototype's 11 entries (6 education, 5 medical) are ported into it. Only `socratic-tutor` has `real: true` + `modelFile`; stubs carry `fileBytes` for believable simulated downloads. The `adapters: []` seam remains documented-but-absent.

**Hero model:** primary candidate **Phi-3.5-mini-instruct Q4_K_M** (~2.2 GB, MIT — cleanest IP story); alternate **Llama 3.2 3B Instruct Q4_K_M** (~1.9 GB). **Decision is empirical:** both run against the hardened Socratic system prompt through llama-server; whichever more reliably withholds the answer and asks one guiding question wins. (Installer-size pressure — see §7 — also favors the smaller file and is part of the same decision.)

**System prompt:** the spec's one-liner expanded and hardened: never reveal the final answer even when asked directly, one question at a time, acknowledge correct steps briefly, keep responses short.

**Covers:** a Node script (`tools/covers/`) generates 11 webp covers, ~640×800 portrait (book-cover feel, fits the "library" identity). One coherent generative visual system: ink background, seeded geometric composition per model id, teal accent for education, amber for medical — the prototype's exact palette. Swap-in seam: replace files in `covers/`.

## 7. Packaging

- `tauri.conf.json`: bundle `resources/` + sidecar via `externalBin`; product name **Cleophis**; identifier `com.cleophis.desktop`; icon set generated (`tauri icon`) from a teal mark adapted from the prototype's logo.
- **Installer: WiX `.msi`** — NSIS installers hard-cap at 2 GB and model+app hovers right at that line; MSI has no such limit and is still a single double-clickable file.
- Expected installer size ~2 GB (model-dominated). Acceptable for the demo per source spec §7.
- **No code signing** (no cert on hand). The demo runs the already-installed app, so SmartScreen never appears on stage. Signing is a post-demo item.
- WebView2: Tauri's installer bootstraps it if missing (default on Win 11 anyway).

## 8. Verification & definition of done

Maps 1:1 to the source spec's DoD, executed on the actual demo machine:

1. **Clean-machine test:** install the `.msi` in Windows Sandbox or a fresh user profile with zero dev dependencies; app launches and full flow works.
2. Cover-forward catalog renders; compatibility badges appear after mock sign-in.
3. Hero model: Get → progress → **slides straight into live chat**, greeting pre-printed, no intermediate step.
4. Scripted Socratic probes (e.g. "solve 2x + 6 = 14", "just tell me the answer") **stream** back guiding questions, never the final answer. This test also decides Phi vs Llama.
5. **Airplane-mode run-through:** wifi off before launch; entire flow works; cost counter reads $0.00 throughout.
6. Installer is a single double-clickable `.msi`.
7. Perf snapshot on the GTX 1650 (first-token latency, tok/s) **and** a forced `-ngl 0` CPU run so plan B is verified before it's needed.

**Testing posture (demo scope, lean):** Rust unit tests for catalog parsing and tier mapping; one smoke test that spawns the sidecar and completes a prompt; the manual DoD checklist above. No test pyramid for demo-scoped code.

## 9. Out of scope (unchanged from source spec §8)

No auth/Supabase (mock sign-in only), no CDN/entitlements/updates, no adapter hot-swap (schema seam only), one real model only, no payments/telemetry, **no macOS build for the demo**. Signing deferred.

## 10. Risks & mitigations

| Risk | Mitigation |
|---|---|
| Vulkan build misbehaves on the 1650 | Verified early (week 1); fallback `-ngl 0` CPU path tested as part of DoD; CUDA build of llama-server is a further option if Vulkan disappoints. |
| Installer > 2 GB breaks tooling | WiX MSI chosen up front; model choice also weighs file size. |
| Model gives answers instead of Socratic questions | Empirical model selection + hardened system prompt; probe script in DoD. |
| Sidecar dies mid-demo | Watchdog respawn + "reconnecting" UI; model pre-warm makes cold-start invisible. |
| WSL/Windows toolchain friction | Repo on Windows FS; builds via interop; verified `node`, `git`, VS 2022 present; `rustup` install is step one of the plan. |
