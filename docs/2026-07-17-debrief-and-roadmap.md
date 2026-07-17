# Cleophis — Demo Build Debrief & Forward Roadmap

Date: 2026-07-17 · State: demo-build complete (PR #1), all DoD items verified on the demo machine

## 1. What was built

**Product.** A Windows desktop app that is the store, the model manager, and the chat window in one. A non-technical user browses a visual catalog, signs in, "pays," and slides directly into a streaming conversation with a language model running entirely on their own hardware — verified working in airplane mode and on a clean machine with zero dev tools.

**Architecture (two processes).**
- `Cleophis.exe` — Tauri 2 shell. Thin Rust core: catalog resolution (`catalog.rs`), hardware detection and device tiering (`hardware.rs`), and sidecar lifecycle (`inference.rs`). Front-end is vanilla HTML/CSS/JS adapted from the storefront prototype, running in WebView2.
- `llama-server.exe` — official prebuilt llama.cpp Vulkan build, spawned at launch on a random loopback port with the bundled model. The webview streams tokens straight from its OpenAI-compatible SSE endpoint; no Rust middleman in the hot path.

**The engine and model.** Llama-3.2-3B-Instruct Q4_K_M (1.88 GiB) chosen empirically over Phi-3.5-mini: a scripted Socratic probe scored Llama 4/4 (it refuses to reveal answers even under "I give up") at 36–45 tok/s on the GTX 1650, while Phi's Q4_K_M physically cannot load into 4 GB VRAM (reproducible Vulkan OOM). Transcripts and the decision record are in `docs/superpowers/model-selection.md` and `probes-*.md`.

**Engine lifecycle (the part that keeps a live demo alive).**
- Pre-warm at launch: the model loads while the user browses, so "download → chat" has zero dead time honestly.
- GPU→CPU fallback: if Vulkan init or the GPU load fails, the engine respawns with `-ngl 0` (~3 tok/s — slow but functional, verified).
- Watchdog: a mid-session crash respawns the engine (conservatively on CPU), capped at 3 strikes before a clean failure state.
- Orphan sweep: an abnormal app exit (task kill/crash) leaves the sidecar holding VRAM; the next launch sweeps stray `llama-server.exe` processes before spawning — tested against a real orphan.
- Shutdown: normal window close kills the child; a race that could orphan it during startup was found in review and closed.

**Front-end flow (user-shaped during checkpoints).**
Catalog of 11 cover-forward cards (generated, clean-IP art; teal education / amber medical) → signed-out clicks dim the card with a "log in or sign up" nudge → sign-in reveals the device chip and per-card compatibility badges ("Runs great" on this machine) → card drawer → **Get: "Processing payment…" → "✓ Paid — downloading…" → progress → the whole view slides into chat** with the tutor's greeting pre-printed and the input focused. Chat streams token-by-token with a live caret, Stop button (aborts cleanly, keeps partial text), a cost counter pinned at $0.00 that pulses after every reply, and a back arrow that returns to the catalog with the model still warm — re-entry is instant with history intact. Stub models simulate download into "My Library" without chat.

**Packaging & verification.** Single double-clickable WiX MSI (2.02 GB, model baked in — engineered under WiX's 2 GiB CAB limit; NSIS was rejected for its 2 GB cap). Unsigned (acceptable: the demo runs the installed app). Full DoD evidence in `docs/superpowers/verification.md`: airplane-mode run, Socratic probes, GPU/CPU perf, clean-machine install.

**Process artifacts.** Approved spec, executable implementation plan, per-task review records, model-selection and probe transcripts, DoD verification record — all in `docs/superpowers/`.

## 2. How "hot swap" is handled today — honestly

Two different "swaps" from the spec, and their current status:

**Adapter hot-swap (base + domain LoRA + personal LoRA) — deliberately NOT implemented.** Per the spec, only the seam exists: the catalog schema reserves an `adapters: []` field per entry, and the tutor's behavior is carried by the system prompt on the base model. Nothing has to be torn out to add it.

**What the implementation path looks like when you're ready:** llama-server natively supports LoRA adapters — load them at spawn (`--lora-scaled <file> <scale>`, repeatable) and re-weight them at runtime via its `/lora-adapters` endpoint without reloading the base model. So the attach plan is:
1. Catalog entries gain `adapters: [{ file, scale, kind: "domain"|"personal" }]`.
2. `spawn_server` appends the adapter flags when the hero entry has adapters.
3. "Hot" swap between models that share the base = one HTTP call to re-scale adapters (milliseconds), not a 9-second model reload. That's the moment the "same engine, many experts" thesis becomes physically real.

**Model-level swap — the machinery exists, the routing doesn't.** `load_model(model_id)` is today an idempotent readiness gate for the single bundled model, kept API-shaped for the multi-model future. The lifecycle code (spawn → health-gate → emit `engine-ready` with the port → kill old) is exactly a local blue-green swap: to switch models post-demo, spawn a second sidecar with the new `-m` on a fresh port, wait for `/health`, flip the port the UI targets, then kill the old process. The front-end already re-reads the port from every `engine-ready` payload, so it would follow the flip automatically.

## 3. Roadmap: the deferred items you now plan to do

Recommended sequencing philosophy, matching your instinct: **complete a thin end-to-end slice first (real account → real payment → real download → chat), then harden it.** A complete-but-thin pipe tells you where the real risks are; hardening first polishes guesses. Concretely, three milestones:

### Milestone A — Supabase: real accounts and entitlements (replaces mock sign-in)
- **Auth:** Supabase email/password to start (matches the existing modal UI almost 1:1); OAuth later. The Rust side stores the session securely (Windows Credential Manager via `keyring`) and refreshes tokens when online.
- **Schema (with RLS on everything):**
  - `profiles` (id ← auth.users, nickname)
  - `devices` (user_id, hw fingerprint, tier snapshot — powers the compatibility badges server-side later)
  - `models` (the catalog, versioned — `catalog.json` becomes the local cache format of this table)
  - `entitlements` (user_id, model_id, source: purchase/trial, expires_at nullable)
  - `downloads` (audit of what was delivered where)
- **Critical constraint — don't break the offline thesis:** cache the session and entitlements locally with a generous offline grace period. The app must still open a purchased model's chat with wifi off; sync when connectivity returns. This is your differentiator — never let auth make the app worse than the demo.
- The current mock `signIn()` and the device chip/badge code are the exact seams: swap the fake with `supabase.auth.signInWithPassword`, keep everything downstream unchanged.

### Milestone B — Payments (the mock phase becomes real)
- Stripe Checkout (hosted page) opened in the system browser from the Get button; a Supabase Edge Function receives the webhook and writes the `entitlements` row; the app polls (or receives a deep-link callback) and proceeds to download. 
- The UI already has the beats: "Processing payment…" → "✓ Paid — downloading…" were built as the demo's mock — the labels stay, the `setTimeout` is replaced by the Stripe round-trip.
- Start with one-time purchases per model; subscriptions ("$20/mo" chips) can follow once entitlement expiry handling exists.

### Milestone C — CDN model delivery (models leave the installer)
- **Distribution:** Cloudflare R2 is the natural fit (zero egress fees matter when every install pulls 2 GB; S3+CloudFront works too). Installer shrinks from 2 GB to ~15 MB.
- **Entitlement-gated URLs:** a Supabase Edge Function checks the entitlement and mints a short-lived signed URL. No permanent public model URLs.
- **Download manager in Rust (the progress bar becomes real):** ranged/resumable requests, SHA-256 verification against the catalog record, atomic rename into the models dir, progress events to the existing UI progress bar. Failure = resume, not restart — 2 GB over hotel wifi will fail mid-flight, plan for it.
- **Blue-green model updates:** new model version downloads alongside the old, health-loads in a second sidecar, port flips, old file deleted — reusing the lifecycle machinery described in §2.
- **App updates:** `tauri-plugin-updater` + signed releases, once code signing (below) is in place.

## 4. Other recommendations, in priority order

**Before anything else (this week):**
1. **Merge PR #1 and tag `v0.1.0-demo`, and keep the built MSI + installed app frozen.** All new work happens on branches; the demo machine stays untouched. Demo day must be immune to the roadmap.
2. **Write the demo run-of-show checklist** (launch once after any crash so the sweep runs; keep Windows animations on; wifi toggle steps; the three Socratic probe questions that are known to land).

**Robustness pass (after the Milestone A–C slice works end-to-end):**
3. **Code signing** — Azure Trusted Signing or an EV cert. Required before any external user installs; kills the SmartScreen warning and is a prerequisite for the auto-updater.
4. **Engine hardening backlog** (all triaged in the final review): job-object child tethering (kills the sidecar even on hard crashes, replacing the sweep-as-primary), port re-pick on respawn, time-windowed crash counter, automated lifecycle tests, per-profile (dev vs prod) asset scope in `tauri.conf.json`, a visible failure surface if the catalog ever fails to load.
5. **Multi-model support** — `unload_model`, disk quota management, and per-model spawn (the respawn flip from §2). This is when the 10 stub cards become real products.
6. **Crash/telemetry decision** — recommend *opt-in only* and aggressively scrubbed, or none at all: "no query ever leaves the device" is the brand; don't let an analytics SDK quietly contradict the pitch.
7. **macOS build** — everything Rust/JS ports; needs a Mac for Metal llama.cpp, `.dmg`, and notarization. The spec's tier logic already handles Apple Silicon.
8. **Fine-tuned hero + adapter stack** — the actual "Socratic fine-tune" and domain/personal LoRAs via the `adapters` seam (§2), turning the system-prompt demo behavior into a defensible product claim.

**One structural suggestion:** as Supabase/Stripe/CDN code arrives, keep the Rust core's rule intact — *the chat hot path never touches the network*. All cloud interaction belongs in a separate module (auth/entitlement/download), so the offline guarantee stays provable by reading one file rather than auditing the whole app.
