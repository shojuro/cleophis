# Cleophis Mobile — Full Build Specification (v1.1)

**Status:** Build-ready spec. Companion to on-device-rag-spec v1.3 (§6 sync, §7 conversation store), the LoRA+Distribution design spec, and the offline-auth design. Where those documents define behavior, this spec defines the mobile delivery of it — it does not re-litigate settled decisions.
**Prime directive unchanged:** inference local, retrieval local, nothing a user types leaves the device. Every mobile decision below is subordinate to that.

**v1.1 changelog (2026-07-24 review round, verified against the repo):** P0 gate raised to full-adapter-stack + on-device probes; adapter fallback ladder; `llama-cpp-2` status corrected (embeddings-only today, generation is greenfield); memory budgets redefined as total app RSS; §4.1 rewritten around a separate signed `apps.json` (the `kind:"app"` design is removed); downgrade guard wording matched to shipped code; on-device layout carries all three adapters; chat template is per-model (floor tier is Llama, not Qwen); Android 14 foreground-service and install-permission details; export includes JSON; English-tutor surface explicitly post-P1. Amended passages are marked `[v1.1]`.

---

## 0. Scope and phase gates

| Phase | Deliverable | Gate to pass |
|---|---|---|
| **P0 — Spike (1–2 wks)** | Tauri 2 Android target builds; llama.cpp **in-process** via llama-cpp-2 FFI; 1B model streams tokens on a real mid-range phone; sqlite-vec + FTS5 statically registered, golden-pack tests green on-device | `[v1.1]` **1B streams tokens at usable speed on the reference low-end device (§3) with the full hero adapter stack (behavioral + contract + voice) mounted via in-process FFI, and the four Stage-5 behavioral probes pass on-device.** Multi-LoRA composition through FFI is a P0 verification item, not a P1 discovery. No unresolved build blockers |
| **P1 — Android alpha (3–5 wks)** | Full chat app: sidebar (§7 parity subset), signed-catalog fetch→verify→download, tier detection, hero base+behavioral adapter, conversation store, signed APK + self-update | Stage-5 behavioral probes pass in-app on-device; airplane-mode suite green; installs+updates on 3 real devices |
| **P2 — iOS TestFlight (2–3 wks, overlaps P1 tail)** | Same app, Metal backend, memory-ceiling tuned, TestFlight external beta (works in mainland China) | Probes + airplane suite green on iPhone; TestFlight review passed; no Jetsam kills in a 20-min session on min-spec iPhone |
| **P3 — Sync (2–4 wks, separable)** | §6 device-to-device: QR pair, mDNS, LAN transport, pack/settings sync (history opt-in) | §6.7 test suite incl. packet-capture zero-non-local assertion |
| **Deferred beyond P3** | Speech loop (whisper.cpp + Piper — Studywise), personal pack builder on mobile (quick-build only when it comes), Play Store & App Store public listings, EU alt-distribution | — |

`[v1.1]` **Adapter fallback ladder (applies to P0 gate and everything downstream):** the product ships the composed stack the Stage-5 gate actually tested — behavioral + contract + voice, in that order. If in-process multi-LoRA proves flaky (order-sensitivity, memory, instability), the measured retreat is behavioral + contract with a **re-gated probe run** — a deliberate decision with fresh evidence, not a starting assumption. The contract adapter is never the one dropped; it is the anti-hallucination half of the system. Voice is the only droppable layer.

**Non-goals for P1/P2, explicit:** sync, speech, pack building, per-chat adapter hot-swap UI, any store listing, tablets-as-special-case (they inherit phone layout), background inference.

## 1. Architecture — the one big change

**Desktop runs llama.cpp as a sidecar process. Mobile cannot** (iOS forbids spawning subprocesses; Android makes them fragile and kill-prone). Mobile therefore moves inference **in-process**: llama.cpp linked as a native library through the `llama-cpp-2` crate, called over FFI from the Rust core.

`[v1.1]` **Honest status of `llama-cpp-2` in this codebase:** it is pinned today only in `crates/kpack-embed` (`= 0.1.151`, behind the `real` feature) and only for **embeddings** (BGE). Nothing in the repo does text *generation* through it — token streaming, sampling, LoRA application, and chat-template rendering over FFI are all greenfield. Consequences:
- The dependency moves to a **single workspace-level pin** shared with kpack-embed; both the embedder and the engine build against the same llama.cpp backend flags (one native library in the process, not two).
- **Hour-one check (P0 fail-fast):** confirm the crate exposes generation + multi-LoRA + chat-template APIs at a pin kpack-embed can also live with. If the pin must move, kpack-embed moves with it, deliberately.

- One `EngineHandle` owning model + adapter stack + context; explicit lifecycle: `load(base, adapters[]) → session(ctx) → stream(tokens) → unload()`. `[v1.1]` `adapters[]` is plural: composition order behavioral→contract→voice, preserving `inference.rs::build_server_args()` semantics, including the per-adapter sha256 gate (`verify_lora_once()` equivalent).
- **This is the majority of the new engineering.** Everything else ports: Rust core, SQLite/sqlite-vec/FTS5 (static registration — the K1 spike result, `docs/superpowers/verification-milestone-kpack.md`, is the mobile gate), catalog client, `cloud/download.rs` (resumable GET + sha256 + atomic rename), `kpack_core::sign` verification, conversation store, §7 windowing.
- Backends: **iOS = Metal** (llama.cpp's best mobile path). **Android = CPU-first**, ARM dotprod/i8mm kernels detected at runtime; GPU (Vulkan/OpenCL) is opportunistic and OFF by default (driver chaos — see Hazards H3). Never let a GPU path be load-bearing on Android.
- Desktop later inherits the in-process engine (kills the sidecar); not in this milestone's scope but code accordingly (no mobile-only assumptions in EngineHandle).

## 2. Device floor, tiers, and budgets

Onboarding profiler (exists for desktop: `hardware.rs` + `tier_select.rs`) extends to mobile: total RAM, available storage, SoC features (dotprod/i8mm), thermal class if available.

| Device RAM | Tier | Model | n_ctx cap | Notes |
|---|---|---|---|---|
| ≤ 4 GB | Unsupported for now | — | — | Honest "not yet" screen + notify-me; do not ship a bad experience |
| 4–6 GB | Floor | 1B Q4 (~0.7 GB) | 2048 | The Xiaomi-with-4GB customer; Android LMK pressure is constant — see H4 |
| 8 GB+ | Workhorse | 3B–4B Q4 (~2–2.5 GB) | 4096 | Mainstream target |
| 12 GB+ / Apple silicon iPhones | Headroom | 4B (8B experimental, not default) | 4096 | 8B on phones is a lab toy until proven |

`[v1.1]` **Budgets are total-app-RSS, not model-file-size.** The desktop sidecar isolates model memory in a separate process; in-process mobile does not. The LMK/Jetsam budget covers model weights + KV-cache + **the WebView UI and its JS heap** — easily several hundred MB before the model loads. All tier budgets above are validated as *total app RSS measured on hardware with the UI up and the model loaded*; P0 produces exactly that measurement per tier. Model-file size is a floor, never the estimate.

**iOS memory ceilings are stricter than totals suggest** (Jetsam limits per-app, commonly ~2–3 GB even on Pro hardware): iOS defaults one tier *lower* than raw RAM implies until measured. Retrieval context (N chunks) follows the RAG spec's per-tier rule (N=3 floor / 5 above).

**Storage preflight:** before any model download — free-space check (model + 20% headroom), warn on <2 GB remaining after; downloads default **Wi-Fi/unmetered only** (override per download), resume across app restarts, charge-recommended notice for big pulls.

## 3. Reference device matrix (buy/borrow these; emulators lie — H1)

- Android floor: a 4–6 GB RAM budget phone (Xiaomi Redmi / Samsung A-series class, 2–4 years old)
- Android mainstream: an 8 GB mid-ranger
- Android weird-OEM: one Huawei/Honor (no GMS) — exercises the no-Play path we claim to serve
- iOS floor: the oldest iPhone the Metal build supports comfortably (measure; likely iPhone 12-class)
- iOS current: any recent standard (non-Pro) iPhone

## 4. Distribution & updates

### 4.1 Android — we are our own store `[v1.1 — rewritten: separate signed app manifest]`

- Signed APK served from `cleophis-dist` via Cloudflare. `[v1.1]` The APK is described NOT inside the model catalog but in a **separate signed file: `apps.json` + `apps.json.sig`** — same pipeline, same curator key, same verify path (`parse_and_verify()` pattern from `catalog_dist.rs`). Rationale: shipped clients parse the existing hero-entry catalog schema; an app release must never be able to break model resolution for old clients, nor force re-signing the model catalog on every APK bump. The two channels also have genuinely different release cadence (app weekly-ish early on; models rarely).
  - `apps.json` entry fields: `version` (integer, monotonic), `versionName`, `sha256`, `url`, `minOsVersion`, `generated_at` (informational).
  - **Downgrade guard applies per-file:** monotonic integer guard on `apps.json`'s `version`, mechanism parity with the shipped `catalog_version` guard (`check_not_downgrade()` + persisted highest-seen). Entries are immutable-versioned like everything else.
  - The v1 `kind: "app"` design is removed.
- **Self-update:** app checks `apps.json` on launch (and daily), sees newer signed APK → download → verify hash → prompt install (never silent). Sideloaded apps do not auto-update; this feature is mandatory in P1.
- **Google developer verification (H8):** Google is rolling out identity verification for apps installed outside Play on certified devices, with **Thailand among the pilot countries** — plan for "sideloading with a registered developer identity," verify current rollout state at P1 start. Register early; don't let this surprise the exact market we care about.
- Play Store later (reach channel, $25 once): same binary, their billing rules only if we sell through them.
- **APK signing key = crown jewel #2** (see §5).

### 4.2 iOS — through the front door
- P2 = **TestFlight**: up to 10k external testers by link, light review, **works in mainland China** without local App Store filings. This is how the friend's family tests on iPhone.
- App Store public listing is a later milestone (review, commission rules, China = local filings — all deferred).
- No consumer sideloading exists for us (EU DMA alt-distribution = EU-only + Apple notarization/fees — deferred; enterprise certs = prohibited for consumers, never touch).
- App package stays small by design: app binary only; models fetch post-install from our CDN (already the architecture; also what App Store size rules want).

## 5. Security

### 5.1 Key inventory (all ceremony-grade items in one table)
| Key | Lives | Loss means | Rules |
|---|---|---|---|
| Curator private key | Founder machine, outside repo, chmod 600, 2 offline backups | Cannot publish verifiable catalogs → forced app update to rotate | Signing step only; dev never holds it |
| **Android APK signing key (NEW)** | Same regime as curator key | **Users can never update again** | Generate at P1 start via ceremony; consider Play App Signing only for the Play channel later — direct-APK channel keys stay ours |
| Apple signing certs/profiles | Apple Developer account (founder) | Recoverable via Apple | Standard handling |
| B2 keys / HF token | Pipeline env only | Revoke+reissue | **Never in the app** — app does public GET only |

### 5.2 Trust chain on device
- Compiled-in production curator pubkey; release builds **compile-time incapable** of trusting test keys (carry-forward rule).
- Catalog fetch → ed25519 verify → per-artifact sha256 → atomic rename. Refuse on any mismatch. `[v1.1]` Downgrade guard: refuse catalogs older than newest-ever-verified — keyed on the **integer `catalog_version`** with persisted highest-seen (`check_not_downgrade()` in `catalog_dist.rs`); `generated_at` is an informational field, not the guard. `apps.json` gets the same mechanism on its own `version` (§4.1). One mechanism, two files; no second monotonic scheme.
- TLS for transport; integrity does NOT depend on TLS (sig+hash do the work), so no cert-pinning complexity in v1.
- Root/jailbreak: **do not block, do not detect-and-punish.** Our threat model already concedes the device owner (offline-auth §6 logic). Document it.

### 5.3 App hardening (cheap, do all)
- Android: `allowBackup=false` — see §6, this is privacy-critical not just hygiene; no `debuggable` in release; native libs extracted per platform norm.
- Screenshots/recents: user-facing toggle "hide app content in app switcher" (FLAG_SECURE / iOS blur) — OFF by default (screenshots are the user's right), one tap for the immigration-documents user.
- OS keystore for the offline-auth verifier + device keypair (already specced; mobile uses Android Keystore / iOS Keychain).

## 6. Privacy invariants — mobile-specific additions

The desktop invariants (§3.5 RAG spec) apply verbatim. Mobile adds three traps that would silently violate "nothing leaves the device" **via the OS itself**:

1. **Android Auto Backup would upload the conversation DB to the user's Google Drive.** `allowBackup=false` + explicit backup rules excluding app data. Non-negotiable, P1.
2. **iOS iCloud backup would upload it to Apple.** Mark models/packs `NSURLIsExcludedFromBackupKey` (size + re-downloadable anyway). Conversation store: **excluded by default**; a settings toggle "include my conversations in device backups" (clearly worded: encrypted-by-Apple-terms, leaves the device) for users who choose it. Default = the promise holds with zero user action.
3. **Crash reporting / telemetry: none.** No Crashlytics, no Sentry-cloud, no analytics SDKs — full stop. Local crash log the user can *choose* to export and email is the support path. Any third-party SDK is an ingress for exfiltration and an instant contradiction of the thesis.

Also: no dynamic permissions requested in P1/P2 beyond notifications (optional, for download-complete) — no mic (speech is deferred), no contacts, no location, ever-without-a-feature. The permissions screen IS a marketing asset: it should be embarrassingly empty. `[v1.1]` One honest asterisk: the direct-APK channel requires the OS-level "install unknown apps" grant for self-update (`REQUEST_INSTALL_PACKAGES` — special access, not a runtime permission). Say so plainly in the same marketing frame: *"the one permission we ask for is the one that lets you update us without Google."*

## 7. On-device layout

```
app-data/
  models/<name>/<quant>/model.gguf        (excluded from backup)
  adapters/behavioral/v1/<base>/adapter.gguf   [v1.1] full stack on device —
  adapters/contract/v1/<base>/adapter.gguf     composition order behavioral→contract→voice,
  adapters/voice/v1/<base>/adapter.gguf        mirroring inference.rs (§0 fallback ladder governs any retreat)
  packs/…                                  (curated only until pack-builder milestone)
  conversations.db                         (backup-excluded by default, §6)
  auth-cache/<user_id>.json                (offline-auth spec)
  catalog-cache/ …                         [v1.1] includes apps.json + highest-seen version state
```
Android: internal app storage only (no shared/SD in v1 — permission + reliability swamp). iOS: Application Support with backup exclusions.

## 8. Engine & chat integration details

- `[v1.1]` **Chat template is resolved per-model, never hardcoded.** The floor tier's hero is **Llama-3.2-1B** — a non-ChatML template family; ChatML applies to the Qwen tiers. The sidecar got this for free via `--jinja` (llama-server reads the template embedded in the GGUF); the in-process engine must reproduce it: read the GGUF's template (or the catalog's per-model declaration) and render accordingly. **Think-strip rule is Qwen-only:** strip `<think></think>` only at start-of-turn stream, never globally, and never on models that don't emit it.
- `[v1.1]` **Adapter stack always-on, full composition** (behavioral→contract→voice) via FFI adapter load — desktop parity, sha256-gated per adapter, fail-closed on any missing/mismatched artifact. §0's fallback ladder is the only sanctioned deviation.
- Generation runs in a foreground-service (Android) / continues while app foregrounded (iOS); **backgrounding mid-generation:** pause gracefully, persist partial turn, resume or cleanly truncate with UI notice — never a corrupted transcript (H5). `[v1.1]` Android 14+ requires a declared foreground-service *type*; none fits on-device inference cleanly — expect `specialUse` with the justification string ("local AI text generation at explicit user request; no network"). Decide and document at P1, not in a Play-review panic later (interacts with H13).
- Thermal: sustained-inference throttle detection (tokens/sec collapse) → surface a gentle "your phone is warming up, responses may slow" rather than mysterious degradation (H6).
- Provenance stamping (model_id, adapter_ids, contract_version) into chat records — parity with desktop schema (`convstore.rs` already carries the columns).

## 9. UI scope (P1)

§7 parity subset: sidebar (day-grouped chats, folders, search, pin/rename/delete), new chat, chat view with citations-collapse pattern from desktop, model/tier badge, download manager screen (progress, pause/resume, storage meter), onboarding = profile device → recommend tier → download hero. Mobile-specific: safe-area/notch handling, keyboard (IME) never obscuring the input, pull-to-dismiss keyboard, font scaling respect. `[v1.1]` Export (§7.5 **MD/TXT/JSON** — the store ships all three; free parity) via OS share sheet. `[v1.1]` The English-tutor surface (reports table, post-session triage, work-on-this chats) is **post-P1** on mobile; the conversation-store schema ports anyway, so the data model is ready when the UI comes. That's all. Resist scope.

## 10. Hazards registry (the "possible hiccups," named and pre-answered)

| # | Hazard | Reality | Mitigation |
|---|---|---|---|
| H1 | **Emulators lie** | Perf, RAM behavior, and NEON/dotprod differ wildly from real devices | Device matrix (§3) from day one; perf numbers only ever quoted from hardware |
| H2 | **16 KB page alignment** | Newer Android (15+/16) devices require 16 KB-aligned native libs; misaligned llama.cpp builds crash on exactly the newest phones | Build with 16 KB alignment flags in P0; add a CI check |
| H3 | **Android GPU drivers** | Vulkan/OpenCL quality varies from great to broken per SoC/driver | CPU-first policy (§1); GPU behind a flag, never default |
| H4 | **Android LMK / iOS Jetsam** | OS kills the app under memory pressure, mid-generation, without ceremony | Tier caps conservative; `[v1.1]` budgets = total app RSS incl. WebView, measured in P0 (§2); iOS defaults one tier down until measured; state persisted every turn; relaunch restores chat cleanly |
| H5 | **Backgrounding mid-generation** | OS suspends/kills backgrounded inference | §8 pause/persist/resume contract |
| H6 | **Thermal throttling** | Sustained inference halves speed on hot phones | Detection + honest UI notice; short-burst chat usage is the natural fit anyway |
| H7 | **Tauri 2 mobile maturity** | Younger than desktop; occasional plugin gaps (file pickers, share sheet) | P0 spike explicitly exercises every OS integration we need; budget small native shims |
| H8 | **Google developer verification** | Rolling out for outside-Play installs on certified devices; Thailand in pilot wave | Register identity early; track rollout; Huawei/AOSP path unaffected |
| H9 | **Old WebView on sideload devices** | Tauri UI runs in System WebView; ancient WebViews break modern CSS/JS | Set a minimum WebView version check at startup with a plain-language fix screen (WebView updates via Play/OEM store even for sideloaded apps) |
| H10 | **APK key loss** | Permanent update lock-out for every installed user | §5.1 ceremony + two offline backups, verified restorable |
| H11 | **Storage-full mid-download / mid-write** | Corrupt artifacts, crash loops | Preflight (§2), atomic renames everywhere (already the pattern), resume support |
| H12 | **TestFlight/App Store review friction** | Reviewer confusion about "downloads executable-looking model files" | Review notes template: models are data files, hash-verified, no code download; precedent apps exist. Budget one rejection round |
| H13 | **Play alignment later** | Play policy on app self-update conflicts inside Play-installed builds | Channel flag: Play builds disable self-update UI (Play handles it); direct builds keep it |
| H14 | `[v1.1]` **Multi-LoRA over FFI unproven** | Sidecar composes 3 LoRAs via `--lora a,b,c`; the in-process equivalent (per-adapter load calls, ordering, memory) is untested on this codebase | P0 gate item (§0); fallback ladder pre-agreed (behavioral+contract, re-gated probes); contract adapter never dropped |
| H15 | `[v1.1]` **llama-cpp-2 pin conflict** | kpack-embed pins `=0.1.151` for embeddings; engine may need newer APIs; one workspace = one version | Hour-one API-coverage check (§1); if the pin moves, embedder revalidates (golden-pack CI catches drift) |

## 11. Testing & acceptance

`[v1.1]` **Each item below carries a stable identifier (A1–A7).** Added in
place, like the `{}`-vs-`null` and `ndk_context` corrections, because these
were the **only requirement set in this spec with no names**: hazards are
H1–H15, sections are §n, decisions are D-1…D-6 and are cited from code. The
acceptance criteria were unnamed prose, referenced as "the §11 kill-restore
test" — and *a mapping cannot be checked mechanically when one side has no
name*. Four times on this branch a test asserted behaviour no task had built;
all four were caught by human review. The IDs are what let
`check-mobile-build.sh` check it instead. Cross-references to hazards are
load-bearing: they make an unbuilt requirement traceable to the hazard it was
meant to mitigate.

- **A1 — Golden-pack + cross-build determinism CI** extended to both mobile targets (the existing suite, cross-compiled).
- **A2 — Airplane-mode suite on device:** full download→verify→chat cycle with radios off after artifact fetch; any socket attempt post-setup = fail.
- **A3 — Behavioral probes in-app** (the Stage-5 four: fake-entity refusal, 5+5=9 pushback, concession, medical boundary) on floor + workhorse devices — the same "did the adapter survive" gate as the desktop slice. `[v1.1]` Run against the full composed stack (§0); a probe run against a reduced stack is a different gate and is labeled as such.
- **A4 — Kill-and-restore:** force-stop mid-generation → relaunch → transcript intact, no corruption (**H4/H5** gate).
- **A5 — Backup-leak test:** trigger Android backup / iOS backup with defaults → assert conversation DB and packs absent from the backup set (**§6** gate — this is the packet-capture test's sibling).
- **A6 — Update path test:** install v1 APK → publish v1.1 to `apps.json` → in-app update succeeds; tampered APK hash → refused; `[v1.1]` older `apps.json` version → refused (downgrade guard).
- **A7 — Battery/thermal soak:** 20-minute continuous session on floor device → no kill, throttle notice fires appropriately, battery drain recorded and sane (**H6** gate).

## 12. Founder handoff needs (blocking items, before/at P1)

1. Android signing key ceremony (with me, same protocol as curator key; two offline backups verified).
2. Google Play developer account ($25) + developer-verification registration when P1 starts (H8) — even before any Play listing.
3. Apple Developer account ($99/yr) at P2 start; TestFlight app record.
4. Reference devices per §3 (buy used; total budget ~$400–600).
5. `[v1.1]` Pipeline PR for the signed `apps.json` + `apps.json.sig` channel (§4.1) — founder signs the first app manifest. (Replaces the v1 `kind: "app"` catalog-schema addition.)

## 13. Explicitly deferred (so nobody "just adds" them)
Speech loop; personal pack builder on mobile; §6 sync (P3, own gate); per-chat adapter switching UI; tablets-optimized layout; Play/App Store public listings; EU alternative distribution; widgets/share-target integrations; any notification beyond download-complete; `[v1.1]` English-tutor surface on mobile (reports/triage/work-on-this — post-P1, §9).
