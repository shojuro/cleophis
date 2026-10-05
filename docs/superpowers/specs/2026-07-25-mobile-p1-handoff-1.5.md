# Continuation handoff — Phase 1.5 (prefix-KV session reuse)

Written by the mobile-p1 instance while frozen for a gate run, deliberately
outside the worktree so it survives independently of the branch and of me.
**Committed verbatim by the successor instance** (sha256 of the source file:
`79bb3d555fc103cf5ff3c7ea6caff7b91140cac917bad71ee1321c54f27a1afd`); only this
paragraph differs from the original, which remains at
`/home/penguinzyue/cleophis-mobile-logs/HANDOFF-1.5.md`. The findings it carries
are summarised in the 1.5 section of
`docs/superpowers/verification-milestone-mobile-p1.md`, which points back here
for the full design.

Worktree: `/mnt/c/Users/JM505 Computers/dev/cleophis-mobile`, branch
`mobile/p1-alpha`. State at time of writing: **HEAD `734df86`, tree clean.**

---

## Where Phase 1 stands

**1.1–1.4 complete.** cfg seam, resource materializer, in-process engine
lifecycle, `Role::Tool` + closed tool registry, streaming tool-syntax
suppressor, calc tool-loop, chat commands, partial-turn flush. Desktop gates
green throughout (test trajectory 266 → 277 → 286 → 297 → 302, no new-code
failures all phase).

**1.5 is half done.** The mechanism landed in `734df86`; the wiring did not.

---

## 1.5 half one — DONE (commit `734df86`)

`crates/kpack-engine/src/llama.rs`. `LlamaSession` gained a `cached:
Vec<LlamaToken>` mirror of what is resident in sequence 0's KV cache. `stream`
now computes the shared prefix against the new prompt, trims the cache beyond
it (`clear_kv_cache_seq`), and decodes only the remaining suffix at absolute
positions. Generated tokens are appended to the mirror as they are decoded.

### The finding that matters most — read before touching this

**The obvious implementation of 1.5 is silently wrong.** The brief says "keep
`EngineSession` alive per chat", which is necessary but NOT sufficient. Before
`734df86`, `stream` re-rendered the full history and decoded from absolute
position 0 every call, tracked no `n_past`, and never cleared the cache. Holding
a session open across turns would therefore have left **stale KV entries from
the previous turn sitting past the end of the new prompt**, where the model
attends to tokens no longer in the conversation. That is a wrong answer, not a
slow one, and nothing would have reported it.

It was masked because `engine_inproc` opens a fresh session per turn and
`EngineHandle::session()` builds a fresh `LlamaContext` (empty cache). So the
pre-1.5 state is **correct and slow** — the pair 1.5 must break without breaking
the first half.

### Two invariants that carry the safety (do not remove)

1. **Trim before extending.** Anything in the cache beyond the shared prefix is
   stale and must be removed. Its absence *is* the bug above.
2. **Always decode at least one token** (`reuse` is capped at
   `prompt_tokens - 1`), so the sampler reads fresh logits. A fully-reused
   prefix leaves the final logits belonging to the previous turn — again silent.

Only tokens that were actually **decoded** are mirrored. The token ending a turn
(EOG, cancel, or the max-tokens cap) is sampled but never fed back, so recording
it would desynchronise the mirror from the real cache.

Rationale for the mirror existing at all: the KV cache is invisible from Rust.
Without a record of its contents there is no way to know which prefix is valid,
and **reusing a cache you cannot describe is how you get corruption instead of
speed.**

---

## 1.5 half two — NOT DONE. The reuse path is currently DORMANT

`src-tauri/src/engine_inproc.rs` still opens a fresh session per `Command::Chat`,
so every turn gets an empty cache and the new code never reuses anything.
Behaviour today is unchanged from 1.4: correct, and slow.

### Why it needs a restructure rather than a small edit

`EngineHandle::session()` returns `Box<dyn EngineSession + '_>` — it **borrows
the handle**. A session therefore cannot be stored beside the handle (that is a
self-referential struct) and cannot outlive a function scope. The thread loop
must hold the borrow inside a nested loop.

An attempt at this was made and **deliberately reverted** rather than left
half-built: the failure modes in the thread that owns the model are deadlocks
and dropped turns, not compile errors, and it was late in that session. The
design below is settled; only the implementation is outstanding.

### The design (already worked out)

1. **`SessionTurns` borrows instead of owning.**
   `struct SessionTurns<'a, 's> { session: &'s mut (dyn EngineSession + 'a), cancel: Arc<AtomicBool> }`
   (currently it owns `Box<dyn EngineSession + 'a>`).

2. **`Command::Chat` carries a `chat_key: Option<i64>`.** The live session's KV
   cache holds *one specific chat's* prefix. A turn for a different chat must
   open a new session — reusing another conversation's prefix is the same
   corruption class as invariant 1. `None` (transient/unsaved turn) never
   reuses. `chat_cmds::chat_stream` already threads a `chat_id: Option<i64>`
   through for the partial-turn flush; pass the same value.

3. **A nested serve-loop holds the borrow.** Sketch:
   ```
   Command::Chat { .. } => {
       let Some(h) = handle.as_mut() else { reply "not loaded"; continue };
       // inner loop owns one session for as long as turns keep arriving for
       // the same chat; returns when the chat changes, on Load/Shutdown, or
       // when the channel closes.
   }
   ```
   Returning a small enum (serve again / reload / shutdown / channel closed)
   keeps the outer loop in charge of the handle, which is required because
   `Load` must be able to `unload()` it — impossible while a session borrows it.
   Dropping the session before re-borrowing is what satisfies the borrow
   checker; assign `None` first, then open the new one.

4. **Session config already exists**: `session_config(&tier)` (2048 n_ctx on
   `low`, 4096 above, spec §2).

### Acceptance

- `cargo ndk -t arm64-v8a -P 24 check -p cleophis --all-targets` clean, zero
  warnings (the standing bar this phase has held all the way through).
- The real proof is **on-device at CP1**: TTFT-with-history is the number 1.5
  exists for. Expect first-token latency to stop growing with conversation
  length; pre-1.5 it reaches 25–40 s mid-chat on the A22.
- A second turn in the same chat must produce a *coherent* answer, not merely a
  fast one — that is the check for invariant 1 having survived.

---

## Environment and process notes the next instance will need

- **The app crate cannot be built on this Linux host.** `keyring`'s
  `linux-native` feature pulls `libdbus-sys`, which needs system dbus headers,
  and the toolchain is sudo-free by founder decision. Desktop verification is a
  steering-side **Windows** service, requested per phase boundary. Phase 3.1's
  planned move of `keyring` off Android does **not** fix this.
- **Pure modules can still be tested locally** by copying them into a scratch
  crate — done successfully for `engine_inproc/tools.rs` (20 tests),
  `tool_loop.rs` (11), and `convstore.rs` (31, by stripping the `#[tauri::command]`
  block and stubbing two app-crate references). Worth doing; it turns "believed"
  into "measured" without waiting for a gate run.
- **Android build:** `docs/superpowers/mobile-tools/build-android-apk.sh`, tees
  to `/home/$USER/cleophis-mobile-logs/`. `cargo ndk … check` does **not link** —
  anything touching new native symbols needs a real APK build to prove symbols
  resolve.
- **Gate-run protocol v2 is in force:** steering records `git status --porcelain`
  + HEAD before and after each run; all four must agree or the run is attributed
  to "live worktree near `<commit>`". **Freeze discipline: when steering says
  "freeze for gate run", stop editing until "thawed."** This exists because two
  earlier runs compiled an in-flight working tree and were attributed to commits,
  producing two rounds of confidently wrong conclusions.
- **Latest APK for the founder:** sha256
  `b01cf017e86035e09c25201af0a0912a614cf340d005db5478c773f33d477a08`,
  355,106,138 bytes — carries the Facet icon and 1.4's chat commands.

## Open items not owned by 1.5

- **Chat does not work on device until 2.1** swaps the frontend transport. The
  commands exist and nothing invokes them. An install reaching `engine-ready`
  and still failing to send is expected, not a regression.
- **2.2 field requirements** (from founder screenshots): sidebar takes ~2/3 of
  phone width; view transition slides ~1/4 and stops (must be *disabled or
  replaced*, not restyled); **status pill must never clip**; the founder could
  not identify the engine-state element at all; post-download transitions flash
  past unreadably.
- **keyring on Android is a silent in-memory mock until 3.1** — "sign-up works"
  is true, "auth works on Android" is not; a force-stop logs the user out.
- **No model download has ever completed→loaded on device.** The engine-load
  path is unexercised on hardware; no claim should be made until a terminal pill
  state is observed.
