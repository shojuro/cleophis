# Med Triage Phase 3 — Device and Shipped Bytes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship the exact bytes the pod gated — the Qwen3-1.7B Q4_K_M base and the v8 f16 LoRA, composed at runtime like every other tile — prove byte identity between the gated and the shipped artefacts, make the on-device engine reproduce the pod's context byte-for-byte, measure the 1.7B on the two reference phones, set the minimum device tier from that measurement, and publish through the signed catalog once, sharing the base artefact with the tutor track.

**Architecture (amended 2026-09-10, founder ruling — spec A15–A18):** The product has never shipped a merged model; every tile composes base + LoRA at runtime and the mobile engine already does so with sha-gated fail-closed loading. The triage pod, the tutor pipeline and the app's bundled server pin the same llama.cpp commit (`3f08ef2c`), so the pod's base Q4_K_M (sha `25162bff…`, the FLOOR_17B pin) and the LoRA GGUF the gate uploaded are the shippable bytes — no merge, no imatrix, and the post-quantisation bar is met by byte identity rather than by a second serve. Packaging reuses the integration branch's `tools/pipeline` (`build_base.py`, `build_adapter.py`, `build_catalog.py`, `sign_catalog.py`, `publish.py`, `verify_published.py`) unchanged. On the product side the engine gains the two things the pod's `llama-server --jinja` had and it did not: the closed empty think block on the assistant turn, and a prefill so a supervised chat is warm before the user types. The device session measures time-to-route and full-reply latency on the A22 and A51 with both runs quoted, on the `armv8-a` floor the founder ruled for the MVP.

**Tech Stack:** Rust in `crates/kpack-engine` and `src-tauri` (app-crate tests on the Windows host; `cargo test -p kpack-engine` runs here), `cargo ndk` for the device harness, `adb` over wireless debugging, the integration checkout's `tools/pipeline/*.py` (venv there) for artefacts and the signed catalog.

**Spec:** `docs/superpowers/specs/2026-09-09-med-triage-mvp-phases-0-3.md` §6 (Phase 3) as amended by §11 A7–A9 and A15–A18, and §7 (release gate). Depends on Phase 1's v8 adapter and chosen prompt, Phase 2's catalog fields and guard, and Phase 0's `release_gate.py`.

## Global Constraints

- The shipped base is the pod's base: `build_base.py`'s Q4_K_M of `Qwen/Qwen3-1.7B` must hash to `25162bffd5a8cf20079f78e6cac079f7b4f8fdd31403dd1a38177f2af450bfa3` (the gate manifests' `base_q4_sha256`, `base_matches_floors: true`); any other sha stops the task. The shipped LoRA GGUF must hash to the v8 gate manifest's `adapter_gguf_sha256`. No artefact reaches the catalog without those two identities recorded.
- `pipeline/pod/gate_on_pod.py` is not edited. Packaging runs from the integration checkout `/mnt/c/Users/JM505 Computers/dev/cleophis` (branch `feat/english-tutor-demo`, whose `tools/pipeline` is current and whose `.env` holds the keys); product code lands on `mobile/p1-alpha` in this worktree. Never `cd` between them inside one command chain; never edit the integration checkout's tracked files.
- The signed dist catalog is one shared, versioned document: the publish bumps `catalog_version` once, carries the tutor track's existing artefacts untouched, and is coordinated with the tutor's pending 1.7B floor promotion so the Qwen3-1.7B base is published ONCE and shared (same sha).
- On device: greedy decode, the catalog `systemPrompt` as the only system content, no greeting turn, no tool preamble (Phase 2 Task 9), and the closed empty think block on the assistant turn (Task 1 here). A prompt-render parity check proves the device string equals the pod string before any timing is believed.
- Device numbers are quoted as pairs (run 1, run 2 immediately after) and per serial; the `[cpu-verdict]` line must read OK before a number is recorded.
- Arch floor: **`armv8-a` in all three homes** (founder ruling 2026-09-10; the worktree's uncommitted flip to `armv8.2-a+dotprod` was reverted by the controller). `cargo clean -p llama-cpp-sys-2 --release --target aarch64-linux-android` before the first Android build of this phase, because the cmake cache does not notice the define change.
- Cost ceiling: no pod time is required by this phase (the gated artefacts already exist on B2); if `build_base.py`'s local conversion is too slow on this machine, one pod run of it is allowed (~$0.30).

## Deviations from spec §6, and why (recorded as spec §11 amendments)

1. **No merge, no imatrix (A15).** Spec P3.2 asked for `merge_quant_on_pod.py` and a post-quant gate of a merged artefact. Runtime composition ships the gated bytes themselves; the `post_quant_delta` bar is reported as 0 with the two shas as its evidence. A merge path is built only if Phase 3's device numbers miss the bars (founder: "measure first").
2. **`minTier`, not `minDevice` (A8).** Phase 2 Task 9 added `min_tier`; the spec's `minDevice` is the same decision.
3. **The route-first sentence is gated in Phase 1 (A5).** It reaches here only if condition D passed.
4. **The prefix cache is measured through `prefill` (A9).** The engine already reuses the KV prefix; Task 1 adds the warm step; Task 3 measures it.
5. **Shared publish (A16).** The base artefact is published once for both tracks; the catalog version is bumped once.

---

### Task 0: Confirm the arch floor the MVP ships on

**Files:**
- Create: `docs/superpowers/verification-milestone-med-triage-device.md` (the device record for this milestone)

- [ ] **Step 1: Confirm the ruling is on disk**

The founder ruled `armv8-a` on 2026-09-10 and the controller reverted the worktree's three homes to HEAD. Confirm:

Run: `python3 docs/superpowers/mobile-tools/check-cpu-floor.py && git diff --stat -- third_party/llama-cpp-sys-2/build.rs docs/superpowers/mobile-tools/vendor-llama-sys-dotprod.sh src-tauri/gen/android/app/src/main/java/com/cleophis/app/CpuSupport.kt`
Expected: `PASS: aarch64-android CPU floor is armv8-a in all 3 homes` and an EMPTY diff for the three files (HEAD `b5f03ae` ships `armv8-a`). If either differs, stop and report — do not edit the floor.

- [ ] **Step 2: Write the milestone header**

```markdown
# Med Triage — device milestone (accumulating)

## 0. Arch floor for the MVP build

**Ruling (founder, 2026-09-10):** the MVP APK is built at `armv8-a` — one binary runs on every ARMv8.0+ phone, the A51 class (spec §3's reference floor) stays supported, and the dotprod speed cost on the A22 is measured in §3 below rather than assumed. The worktree's uncommitted flip to `armv8.2-a+dotprod` (the gen-12 dispatch experiment) was reverted; `DlNamespaceProbe.kt` stays untracked as that experiment's record. Before the first Android build of this phase: `cargo clean -p llama-cpp-sys-2 --release --target aarch64-linux-android`. The shipped binary's own `[kernels]` line on device is the evidence, never `flags.make`.
```

Commit: `git add docs/superpowers/verification-milestone-med-triage-device.md && git commit -m "P3.0: the MVP ships on the armv8-a floor (founder ruling), recorded in the device milestone"`

---

### Task 1 (product repo): engine parity — the closed think block, and prefill

**Files:**
- Modify: `crates/kpack-engine/src/backend.rs` — `SessionConfig.generation_prefix: Option<String>`; `EngineSession::prefill`
- Modify: `crates/kpack-engine/src/llama.rs` — append the prefix after `apply_chat_template`; implement `prefill`
- Modify: `crates/kpack-engine/src/mock.rs` — `prefill` for the mock
- Create: `crates/kpack-engine/src/render.rs` — pure `with_generation_prefix(prompt, prefix)` and tests
- Modify: `src-tauri/src/catalog.rs` — `generation_prefix: Option<String>` on `CatalogEntry`
- Modify: `src-tauri/src/engine_inproc.rs` — `session_config` sets `generation_prefix` from the hero; `Command::Warm`
- Modify: `src-tauri/src/engine_inproc/serve.rs` and `src-tauri/src/engine_serve.rs` — route `Warm` like `Chat` (prefill, no tokens)
- Modify: `src-tauri/src/chat_cmds.rs` — `chat_warm(chat_id)` command; register in `lib.rs`
- Modify: `src/app.js` — call `chat_warm` when a supervised chat opens
- Modify: `src-tauri/resources/catalog.triage.json` — `"generationPrefix": "<think>\n\n</think>\n\n"`

**Interfaces:**
- Produces: `with_generation_prefix(prompt: &str, prefix: Option<&str>) -> String`; `EngineSession::prefill(&mut self, messages: &[ChatMessage]) -> Result<usize, EngineError>` (prompt tokens decoded); `Command::Warm { convo, chat_key, reply: Sender<Result<usize, String>> }`; Tauri command `chat_warm(chat_id: i64, messages: Vec<WireMessage>) -> Result<usize, String>`.

- [ ] **Step 1: Why (state it in the code and here)**

The pod serves with `--jinja` and `enable_thinking:false`; Qwen3's template then ends the generation prompt with `<|im_start|>assistant\n<think>\n\n</think>\n\n`. The engine renders with `llama_chat_apply_template` (non-jinja), which ends at `<|im_start|>assistant\n`, so on device the model may open its own `<think>` and spend tokens reasoning that the gate never saw, and the engine's `ThinkStripper` hides that cost from the user but not from the clock. Appending the closed block reproduces the pod's context and makes time-to-route measurable against the gate's distribution.

- [ ] **Step 2: Pure function and test**

```rust
// crates/kpack-engine/src/render.rs
//! The one string transform between "the model's own template" and "what the
//! gate fed the model": a catalog entry may pin a GENERATION PREFIX that is
//! appended after the assistant header. For Qwen3 with thinking disabled that
//! is the closed empty think block the jinja path emits and the legacy path
//! does not. Pure and desktop-tested; the real backend calls it once.

/// Append `prefix` to a rendered generation prompt. `None` returns the prompt
/// unchanged, so every entry that pins nothing is byte-identical to before.
pub fn with_generation_prefix(prompt: &str, prefix: Option<&str>) -> String {
    match prefix {
        Some(p) if !p.is_empty() => {
            let mut s = String::with_capacity(prompt.len() + p.len());
            s.push_str(prompt);
            s.push_str(p);
            s
        }
        _ => prompt.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn none_is_the_identity() {
        assert_eq!(with_generation_prefix("<|im_start|>assistant\n", None), "<|im_start|>assistant\n");
        assert_eq!(with_generation_prefix("x", Some("")), "x");
    }

    #[test]
    fn the_qwen3_no_think_block_lands_after_the_assistant_header() {
        let out = with_generation_prefix("<|im_start|>assistant\n", Some("<think>\n\n</think>\n\n"));
        assert_eq!(out, "<|im_start|>assistant\n<think>\n\n</think>\n\n");
    }
}
```

Add `pub mod render;` to `lib.rs` and `pub use render::with_generation_prefix;`.

- [ ] **Step 3: SessionConfig and the real backend**

`backend.rs`: add `pub generation_prefix: Option<String>` to `SessionConfig` (default `None`). The struct is built by literal at three sites, and each needs the field: `crates/kpack-engine/examples/probe.rs:104` (from the new `--generation-prefix` flag, Task 4), `crates/kpack-engine/examples/stream.rs:85` (same flag), `src-tauri/src/engine_inproc.rs:681` (`session_config`, from the hero entry, Step 4). Then add to the `EngineSession` trait:

```rust
    /// Decode `messages` into the KV cache WITHOUT sampling, so the next
    /// `stream` on this session pays only for its suffix. Returns the number of
    /// prompt tokens now resident. A supervised chat calls this when it opens.
    fn prefill(&mut self, messages: &[ChatMessage]) -> Result<usize, EngineError>;
```

`llama.rs`: in `stream_turn`, after `let prompt = self.model.apply_chat_template(...)?;` insert `let prompt = crate::render::with_generation_prefix(&prompt, self.cfg.generation_prefix.as_deref());`. Factor the render-tokenise-trim-decode section (from `let chat: Vec<LlamaChatMessage>` through `self.cached.extend_from_slice(&tokens[reuse..]);`) into `fn decode_prompt(&mut self, messages, add_generation_prompt: bool) -> Result<(usize, LlamaBatch), EngineError>` and implement:

```rust
    fn prefill(&mut self, messages: &[ChatMessage]) -> Result<usize, EngineError> {
        // Render WITH the assistant header and prefix, exactly as the turn will,
        // so the whole prompt up to the user's next words is a shared prefix.
        let (prompt_tokens, _batch) = self.decode_prompt(messages, true)?;
        Ok(prompt_tokens)
    }
```

with the invalidate-on-error wrapper the `stream` impl already uses. `mock.rs`: `prefill` returns the rendered message count.

- [ ] **Step 4: Catalog, engine, command, FE**

`catalog.rs`: `#[serde(default)] pub generation_prefix: Option<String>,` on `CatalogEntry`; the triage entry gains `"generationPrefix": "<think>\n\n</think>\n\n"`; the catalog test asserts it. `engine_inproc.rs`: `session_config` sets `generation_prefix: crate::inference::hero_entry(app).and_then(|e| e.generation_prefix)`. `serve.rs`: add `Command::Warm(WarmTurn { convo: Vec<LoopMessage>, chat_key: Option<i64>, reply: mpsc::Sender<Result<usize, String>> })`; in `engine_serve.rs`'s `serve_loop`, route `Warm` exactly as `Chat` for session reuse (same `chat_key` rule) but call `session.prefill(&rendered)` and reply the count. `chat_cmds.rs`:

```rust
/// Warm a supervised chat's session: decode the pinned system prompt (and any
/// history) so the first reply pays only for the user's turn. Fire-and-forget
/// from the FE; a failure is logged, never shown.
#[tauri::command]
pub async fn chat_warm(chat_id: i64, messages: Vec<WireMessage>, app: AppHandle) -> Result<usize, String> {
    #[cfg(mobile)]
    { return imp::chat_warm(chat_id, messages, app).await; }
    #[cfg(desktop)]
    { let _ = (chat_id, messages, app); Err(DESKTOP_REFUSAL.to_string()) }
}
```

with `imp::chat_warm` building `to_loop_messages(app, wire)` and sending `Command::Warm` keyed by `Some(chat_id)`. Register `chat_cmds::chat_warm` in `lib.rs`. `src/app.js`: in `openChat` and after `create_chat` in `sendCompletion`, when `state.chat.model.supervised`, call `invoke('chat_warm', { chatId, messages: assembleMessages({ entry: m, groundedPrompt: null, sent: [], ungroundedNote: '' }).messages }).catch(() => {})`.

- [ ] **Step 5: Tests and the parity check**

Run here: `cargo test -p kpack-engine` (render tests, mock prefill, prefix arithmetic). On Windows: `cargo test -p cleophis catalog`. On device (Task 3): add `--dump-prompt` to `crates/kpack-engine/examples/stream.rs` that prints the rendered prompt string the session would decode (call the same `decode_prompt` path with a debug hook, or re-render with `apply_chat_template` + `with_generation_prefix` in the example) for `--system <pinned> --prompt PARITY-CHECK-USER-TURN --generation-prefix "<think>\n\n</think>\n\n"`; `sha256sum` it and compare to the pod's rendering of the same prompt: the v8 gate's `llama-server --jinja` `/apply-template` output for `[system: pinned prompt, user: PARITY-CHECK-USER-TURN]` with `enable_thinking:false` — obtain it once by serving the base Q4 locally with the same llama.cpp commit (`tools/pipeline/llama.cpp` at b10042 in the integration checkout, `llama-server --jinja -m <base-q4> --chat-template-kwargs '{\"enable_thinking\":false}'`, then POST `/apply-template`), and record its sha in the milestone document. They must be equal before any timing in Task 4 is recorded. If they differ, print both and fix the rendering, never the comparison.

Commit both repos' changes: `"P3.2: generation prefix reproduces the pod's no-think context; prefill warms a supervised chat"`.

---

### Task 2 (integration checkout + triage repo): package the gated bytes, prove identity

**Files:**
- Create (integration checkout `tools/pipeline/work/out/`): `models/Qwen3-1.7B-Q4_K_M.gguf` + `.manifest.json` via `build_base.py`; `adapters/triage-v8-Qwen3-1.7B.gguf` + manifest via `build_adapter.py`
- Create (triage repo): `artifacts/v8-shipped-bytes.json`

**Interfaces:**
- Consumes: `tools/pipeline/build_base.py --repo Qwen/Qwen3-1.7B --quant Q4_K_M --model-name Qwen3-1.7B --license apache-2.0`; `tools/pipeline/build_adapter.py` (reads the PEFT zip from the private bucket; read its `--help` for the key/name flags); the v8 gate's `run-manifest.json` (`work/gate-17b-v8/run-manifest.json` in the triage repo: `base_q4_sha256`, `adapter_gguf_sha256`, `adapter_zip_key`, `adapter_zip_sha256`) and R77's shipping arm (`v8` or `v8w`).
- Produces: two manifests in the shape `build_catalog.py` consumes (`kind: base` / `kind: adapter`, `base_model: Qwen3-1.7B`, sha256, size, license) and `artifacts/v8-shipped-bytes.json` recording every sha and the identity verdicts.

- [ ] **Step 1: The base**

```bash
cd "/mnt/c/Users/JM505 Computers/dev/cleophis/tools/pipeline"
.venv/bin/python3 build_base.py --repo Qwen/Qwen3-1.7B --quant Q4_K_M --model-name Qwen3-1.7B --license apache-2.0
sha256sum work/out/models/Qwen3-1.7B-Q4_K_M.gguf
```

Expected: `25162bffd5a8cf20079f78e6cac079f7b4f8fdd31403dd1a38177f2af450bfa3`. That equality is the whole point: the pipeline's converter + quantiser at b10042 reproduce the pod's base byte for byte. If the sha differs, STOP — record both shas in the report and do not proceed; the difference (converter revision, quant args) has to be found before anything ships. (If the tutor track has already published a Qwen3-1.7B base, compare against its catalog sha too: equal means one shared artefact.)

- [ ] **Step 2: The adapter**

```bash
.venv/bin/python3 build_adapter.py --help    # find the flags for: bucket key of the PEFT zip, base snapshot/repo, output name
.venv/bin/python3 build_adapter.py <flags naming lineage/cleophas-triage/v8/adapters/triage-armb-<v8|v8w>-Qwen3-1.7B.zip, base Qwen/Qwen3-1.7B, output adapters/triage-v8-Qwen3-1.7B.gguf>
sha256sum work/out/adapters/triage-v8-Qwen3-1.7B.gguf
```

Expected: equal to the v8 gate manifest's `adapter_gguf_sha256` for the shipping arm. If `build_adapter.py`'s conversion flags differ from `gate_on_pod.py`'s (`convert_lora_to_gguf.py --outtype f16 --base <snapshot>`), the shas differ: in that case fetch the gate's own uploaded GGUF instead (`lineage/cleophas-triage/v8/gate/17b/<stack>-armb.gguf` — the key `gate_on_pod.py` uploads at its line ~286) with a small boto3 fetch, verify its sha against the manifest, and write its manifest by hand in `build_adapter.py`'s output shape. Either way the shipped LoRA is the gated LoRA, and the report says which path produced it.

- [ ] **Step 3: Record identity and satisfy the post-quant bar**

Write `artifacts/v8-shipped-bytes.json` in the triage repo:

```json
{"schema": "cleophas-triage/shipped-bytes/v1", "recorded_utc": "<utc>", "arm": "<v8|v8w>",
 "base": {"file": "Qwen3-1.7B-Q4_K_M.gguf", "sha256": "25162bff…", "equals_gate_base_q4_sha256": true, "equals_floor_17b": true},
 "adapter": {"file": "triage-v8-Qwen3-1.7B.gguf", "sha256": "<sha>", "equals_gate_adapter_gguf_sha256": true, "produced_by": "build_adapter.py | fetched from gate prefix"},
 "post_quant_delta": {"value": 0, "evidence": "the shipped bytes are the gated bytes (both shas equal); no second serve; spec A15"}}
```

Then evaluate the release bars on the v8 gate transcripts with the delta supplied: `python3 -m pipeline.analysis.release_gate --gate-dir work/gate-17b-v8 --stack Qwen3-1.7B-armb-<arm>.pW --endpoint-json work/gate-17b-v8/endpoint-<arm>-pW.json --arm armA --post-quant-delta 0 --bars release --json artifacts/v8-release-gate.json`. Both verdict files are committed (triage repo): `git add artifacts/v8-shipped-bytes.json artifacts/v8-release-gate.json && git commit -m "P3.2: the shipped bytes are the gated bytes — base and adapter shas proven equal; release bars evaluated with post_quant_delta 0 by identity"`.

---

### Task 3 (product repo): measure on the A22 and the A51

**Files:**
- Modify: `crates/kpack-engine/examples/probe.rs` — a `triage` probe set and `--generation-prefix`
- Create: `docs/superpowers/mobile-tools/run-triage-device.sh`
- Modify: `docs/superpowers/verification-milestone-med-triage-device.md`

- [ ] **Step 1: The triage probe set**

In `probe.rs`, add beside `STAGE5`:

```rust
// Four triage prompts spanning the four dispositions, patient voice, no
// diagnosis named — the device's Stage-5 equivalent for the supervised entry.
const TRIAGE: &[Probe] = &[
    Probe { set: "triage", label: "emergency", q: "crushing pain in the middle of my chest for the last hour, going down my left arm, and i'm sweating buckets" },
    Probe { set: "triage", label: "clinician", q: "i've had a dull ache low on the right of my belly since yesterday and i feel a bit hot, but i'm walking about fine" },
    Probe { set: "triage", label: "self-care", q: "sore throat and a runny nose for two days, no temperature, eating and drinking normally" },
    Probe { set: "triage", label: "out-of-scope", q: "i'm 30 weeks pregnant and the baby has moved less than usual today" },
];
```

`"triage" => TRIAGE.iter().collect(),` in the `match a.set`, a `--generation-prefix` flag stored into `SessionConfig { generation_prefix, .. }`, and print `[t2r {:.2}s]` — the wall time from `stream` start until the streamed text first resolves to a route. Resolving the route in Rust would be a port; instead print the first 12 tokens' arrival time (`first_n_at`) and let the device log be scored afterwards by `node -e` over the saved transcript with `src/triage/detectors.mjs`' `detectRoute` on growing prefixes. Print `[tokens 12 at {:.2}s]` alongside tok/s and RSS.

- [ ] **Step 2: The device script**

`run-triage-device.sh` wraps `run-on-device.sh`'s discovery and push (copy `discover_devices`, `adbs`, `push_to` verbatim; this is a third copy of those functions, so record the duplication in the milestone document's debt list), then per device:

```bash
SYS="$(cat "$ROOT/../cleophas-triage/probes/prompt-pinned.txt")"   # or the path passed with --prompt-file
for run in 1 2; do
  echo "== [$serial] run $run (base Q4_K_M + v8 LoRA, greedy, triage set) =="
  adbs "$serial" shell "cd $DEV && LD_LIBRARY_PATH=$DEV ./probe --model $(basename "$MODEL") --model-sha $MODEL_SHA --behavioral $(basename "$ADAPTER") --behavioral-sha $ADAPTER_SHA --system \"$SYS\" --template chatml --generation-prefix '<think>\n\n</think>\n\n' --temp 0 --max-tokens 320 --n-ctx 2048 --set triage" 2>&1 | tee "$LOGDIR/$serial-run$run.log"
done
adbs "$serial" shell "cd $DEV && LD_LIBRARY_PATH=$DEV ./stream $(basename "$MODEL") --system \"$SYS\" --template chatml --temp 0 --prompt PARITY-CHECK-USER-TURN --dump-prompt" | sha256sum
```

`MODEL` is the base Q4 from Task 2 (`work/out/models/Qwen3-1.7B-Q4_K_M.gguf` in the integration checkout; copy to `~/cleophis-artifacts/`) and the adapter is passed with `--behavioral <adapter.gguf> --behavioral-sha <sha>` (the probe and stream examples already take these; the engine composes at load with the sha gate). `MODEL_SHA` and the adapter sha come from `artifacts/v8-shipped-bytes.json`. Build first: `cargo clean -p llama-cpp-sys-2 --release --target aarch64-linux-android` (the floor is `armv8-a`, Task 0), then `cargo ndk -t arm64-v8a -P 24 build --release --features real --example stream --example probe`. The founder's existing `docs/superpowers/mobile-tools/run-on-device.sh` already discovers devices and pushes the harness; this script reuses its functions verbatim.

- [ ] **Step 3: The founder device session**

Order, cold phone first: (1) `[cpu-verdict]` OK on each device (INCOMPATIBLE on the A51 under a dotprod build is a Task 0 outcome, recorded); (2) parity sha equals the pod-side rendering recorded in Task 1; (3) run 1 then run 2 immediately; (4) `VmRSS`/`VmHWM`; (5) the APK (`--variant=triage`) with the tile downloading the published artefacts (Task 5) — the `[triage] time-to-route` and `[triage] route` lines from logcat over the four prompts in a fresh chat, twice.

The prefix-cache measurement (spec P3.2) comes out of step 3 without a separate run: the four probes share the pinned system prompt in one session, so the first probe's prefill time is "before" and the second to fourth are "after" the prefix is resident. Quote both. The probe prints the prompt token count; the spec's "60-token user turn" is approximated by the emergency probe, and the count printed is the one recorded.

Record in the milestone document, per serial:

| device | build | cpu-verdict | parity | run | load s | prefill s | tok/s per prompt | 12-tokens-at s | RSS/HWM MB | route per prompt |
|---|---|---|---|---|---|---|---|---|---|---|

and the app's `time-to-route` and full-reply times, p95 over the eight app prompts.

- [ ] **Step 4: Commit**

`git add crates/kpack-engine/examples/probe.rs docs/superpowers/mobile-tools/run-triage-device.sh docs/superpowers/verification-milestone-med-triage-device.md && git commit -m "P3.1: triage probe set and device script; A22/A51 measured in pairs with the parity check first"`

If time-to-route or full-reply p95 miss the device bars on the A22 under runtime composition, that is the trigger for the merge path the founder deferred (spec A15): record the numbers, stop, and raise it — do not build the merge inside this task.

---

### Task 4 (product repo): the minimum tier, from the measurement

**Files:**
- Modify: `src-tauri/resources/catalog.triage.json` — `minTier`
- Modify: `docs/superpowers/verification-milestone-med-triage-device.md` — the decision

- [ ] **Step 1: Apply the pre-registered rule**

From `artifacts/mvp-release-gate-prereg.json` `device_bars`: time-to-route ≤ 10 s and full reply p95 ≤ 30 s, second-run degradation ≤ 30%. If the A22 meets all three, `minTier: "low"`. If not, `minTier: "mid"` and the A22 class is below the line for the MVP; the FE hides Get below `minTier` (Phase 2 Task 9 Step 5). Write the decision with the two runs' numbers beside it and the fallback named (server-side inference, post-MVP).

- [ ] **Step 2: Commit**

`git add src-tauri/resources/catalog.triage.json docs/superpowers/verification-milestone-med-triage-device.md && git commit -m "P3.3: minTier set from the A22/A51 measurement"`

---

### Task 5 (integration checkout + product repo): publish once, pin the catalog

**Files:**
- Modify (integration checkout, tracked — the ONE exception to "never edit"): nothing, unless `verify_published.py`'s own allowlist lacks `Qwen3-1.7B` (check with `grep -n "Qwen3-1.7B" tools/pipeline/verify_published.py`; `build_catalog.py`'s `KNOWN_BASE_MODELS` already has it on both branches)
- Modify (this worktree): `src-tauri/resources/catalog.triage.json` — pin `sha256`/`fileBytes` of the base and `adapterFile`/`adapterSha256`/`adapterId` of the v8 LoRA; `version: 2`; `promptFingerprint` = the M5 winner's; `systemPrompt` = its text
- Modify: `src-tauri/src/catalog.rs` tests — the triage hero has BOTH `model_file` and `adapter_file` (the tutor's shape)

- [ ] **Step 1: Coordinate the shared publish**

Read the live dist catalog (`verify_published.py --pubkey <curator pubkey hex>` prints it) and check whether a `Qwen3-1.7B` base artefact is already published by the tutor track's floor promotion. If yes and its sha equals `25162bff…`, reuse it — the triage entry pins the same path; publish only the adapter. If no, this publish adds it, and the tutor promotion later reuses it. Record which case applied in the milestone document.

- [ ] **Step 2: Build, sign, publish, verify (per `tools/pipeline/README.md`)**

```bash
cd "/mnt/c/Users/JM505 Computers/dev/cleophis/tools/pipeline"
.venv/bin/python3 build_catalog.py --help   # the flags that add manifests to the current published catalog (--base-url mode increments catalog_version)
.venv/bin/python3 build_catalog.py <flags: --base-url <public catalog url>, the base manifest (only if not already published), the adapter manifest>
.venv/bin/python3 sign_catalog.py --catalog work/catalog/catalog.json
.venv/bin/python3 publish.py --catalog work/catalog/catalog.json --out-dir work/out --dry-run
.venv/bin/python3 publish.py --catalog work/catalog/catalog.json --out-dir work/out --verify-pubkey <curator pubkey hex>
.venv/bin/python3 verify_published.py --pubkey <curator pubkey hex>
```

The last prints `OK` with the new artefacts' shas and sizes verified from the public path, and the tutor's existing artefacts unchanged.

- [ ] **Step 3: Pin the catalog in the app**

In `catalog.triage.json`'s `med-triage` entry: `modelFile: models/Qwen3-1.7B-Q4_K_M.gguf`, `sha256: 25162bff…`, `fileBytes: <size>`, `adapterFile: adapters/triage-v8-Qwen3-1.7B.gguf`, `adapterSha256: <sha>`, `adapterId: triage-v8-Qwen3-1.7B`, `version: 2`. The test in `catalog.rs` asserts both files and both shas are present and 64-hex. On Windows: `cargo test -p cleophis catalog`.

- [ ] **Step 4: Record the shipped-bytes gate**

The catalog's two shas equal `artifacts/v8-shipped-bytes.json`'s, which equal the v8 gate manifest's — write the three-way identity and `artifacts/v8-release-gate.json`'s output side by side in the milestone document. Phase 4 (certification, `TRIAGE_BANK=cert`) runs `gate_on_pod.py` unchanged against the same adapter zip, which serves the same bytes.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/resources/catalog.triage.json src-tauri/src/catalog.rs docs/superpowers/verification-milestone-med-triage-device.md
git commit -m "P3.4: the triage tile pins the gated base and LoRA shas; published once through the signed catalog, base shared with the tutor track"
```

---

## Exit criteria for Phase 3

- `artifacts/v8-shipped-bytes.json`: base sha equals `25162bff…` and the adapter sha equals the v8 gate manifest's; `artifacts/v8-release-gate.json` evaluated with `post_quant_delta 0` by identity.
- `cargo test -p kpack-engine` green here; `cargo test -p cleophis` green on the Windows host.
- The device milestone document has, per serial: `[cpu-verdict]` OK on the `armv8-a` build, parity sha equal to the pod-side rendering, two runs, RSS, and the app's time-to-route and full-reply p95; `minTier` set from it by the pre-registered rule; if the A22 misses a device bar, the merge path is raised to the founder with the numbers, not built.
- `verify_published.py` prints `OK` for a catalog that carries the tutor's artefacts unchanged plus the triage adapter (and the base, unless already shared); the app's catalog pins the same two shas.
- Phase 4 can start: `probes/cert-lock.mjs --claim Qwen3-1.7B-armb-<arm>.pW` and `gate_on_pod.py` with `TRIAGE_BANK=cert CERTIFY_SHA=<prereg sha> N_TRIAGE=300` on the same adapter zip.
