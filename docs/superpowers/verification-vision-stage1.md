# Vision input, Stage 1 (engine integration): verification record

Branch `mobile/vision-p1`, head at the time of the gates `4a819d3`. Plan:
`docs/superpowers/plans/2026-11-01-vision-stage1-engine.md`. Stage 0's record is
`docs/superpowers/verification-vision-spike.md`. Every number below comes from a command
run in Task 10 (date 2026-11-01, WSL2 box and the founder's Windows toolchain). Nothing is
estimated unless marked.

## What each task built

| Task | Commit | What |
|---|---|---|
| 1 | `b695406` | The idefics3 one-pass switch is a C-linkage setter (`cleophis_mtmd_idefics3_overview_only`, atomic exchange) instead of an environment variable. Adds a fifth outline-atomics helper. |
| 2 | `a821cb9` | `vision_prep`: size caps before decode, EXIF orientation 1-8, downscale to the encoder's edge. |
| 3 | `fb2ad57`, `958dcfa`, `b2caf88` | `vision_parse`: DocTags to blocks, exp-mean confidence, operator floor, verdict. Bounded nesting, NaN fails closed, tag stripping is a fixpoint, prose stays `text`. |
| 4 | `e44cf27` | `VisionBackend`/`VisionHandle` in the DMZ; `prepare_vision_stack` gates base and projector (exists, then sha). |
| 5 | `d7c6f1c` | `LlamaVision`, the Stage 0 sequence as a `VisionHandle`, behind the `vision` feature. |
| 6 | `266e910` | `TierVariant.vision`, the pin check and keep-list cover the transcriber; `dist-pick.js` picks `vision_base` + `mmproj`. |
| 7 | `35e3dbb` | `resolve_vision` / `vision_hashes` (fail-closed), `EngineInfo.vision`, the co-residency rule as a tested policy. |
| 8 | `c6ea3e7`, `ab3cc44` | The inference thread serves `Transcribe` and `PrewarmVision`; sequential below 6 GB RAM; the transcriber is released 120 s after its last use, even mid-chat. |
| 9 | `4a819d3` | `transcribe_image`, `prewarm_vision`, `vision_state`: under the foreground service, cancellable. |

## Gates

| Where | Command | Last line |
|---|---|---|
| WSL | `python3 docs/superpowers/mobile-tools/check-cpu-floor.py` | `PASS: aarch64-android CPU floor is armv8-a in all 4 homes` |
| WSL | `cargo test -p kpack-engine` | `test result: ok. 149 passed; 0 failed` (lib), `4 passed` (parity), 0 doc tests |
| WSL | `npm test` | `# pass 1215`, `# fail 0` |
| WSL | `RUSTFLAGS='-D warnings' cargo ndk -t arm64-v8a -P 24 check -p cleophis --all-targets --release` | `Finished \`release\` profile [optimized] target(s) in 5.04s`, exit 0 (the objects were already built by the per-task checks) |
| Windows | `cargo test --locked -p cleophis -- --test-threads=1` on a detached worktree at `4a819d3` | `test result: ok. 452 passed; 0 failed; 10 ignored`, then three more `ok` result lines (bin 0, `engine_smoke` 0 passed 1 ignored, doc tests 0), `== exit 0 ==` |

`cargo test -p cleophis` does not build on the WSL box (glib and dbus development
libraries are absent, no sudo), so the desktop suite ran on the Windows toolchain. Its log
shows the Stage 1 tests executing, not only compiling: the serve routing tests, the
catalog vision tests including `a_tier_without_vision_still_parses_and_resolves_none`, the
`vision_cmds` wire / base64 / in-flight tests and the three `vision_policy` tests. The 10
ignored tests are the pre-existing device and sidecar gated ones.

## The shipping library

Built with `cargo ndk -t arm64-v8a -P 24 build --release -p cleophis --lib`
(`Finished \`release\` profile [optimized] target(s) in 17m 12s`, exit 0).

| | |
|---|---|
| Artifact | `~/cleophis-mobile-target/aarch64-linux-android/release/libcleophis_lib.so` (not stripped) |
| sha256 | `6b28b0328083d0cebe47f4cec6d2e307965fe50b78b7dacd5f3b9dfd5b2fa4ed` |
| Size | 40,041,448 bytes |
| Arch audit | `PASS: …/libcleophis_lib.so — nothing above the floor outside 5 outline-atomics helper(s)` |

- **Helper count is 5 from Task 1 on.** The setter's atomic exchange adds
  `__aarch64_swp4_acq_rel`, runtime-gated on LSE like the other four. Stage 0's
  measurement lines in `verification-vision-spike.md` say 4 because they were measured
  before Task 1; they stay as measured.
- **Size delta is not available.** No pre-vision library size was ever recorded, and this
  task did not build a baseline. 40,041,448 bytes is the first measurement; the next
  build is the one that can be compared.
- **The vendored setter survives into the product library.**
  `llvm-nm -D … | grep -c cleophis_mtmd_idefics3_overview_only` prints `0`, because the
  cdylib does not export it dynamically. The full symbol table (`llvm-nm` without `-D`)
  has it: `000000000116d330 t cleophis_mtmd_idefics3_overview_only`, beside the static
  `_ZL33g_cleophis_idefics3_overview_only` (`b`) it guards, and the Rust declaration in
  `crates/kpack-engine/src/vision.rs:38` is the caller (`:105`, `:107`). This closes the
  Task 1 review item: the symbol is linked and local, which is what a one-TU switch
  should be. The check Task 1 asked for (`-D`) cannot pass and should not be repeated.
- **No APK was built.** `build-android-apk.sh` needs signing material. The APK and device
  steps are owed to Stage 2's checklist.
- **The align-probe now links the vision objects.** `docs/superpowers/mobile-tools/aarch64-align-probe/Cargo.toml`
  has `features = ["real", "vision"]`, so `check-mobile-build.sh`'s arch audit (which
  audits the probe `.so`, and ends with `audit-arch-floor.sh`) covers the multimodal
  objects. `bash -n check-mobile-build.sh` is clean. The script itself was not run: it
  cross-compiles for minutes.

## Device check

Not run. `~/bin/adb-win devices` lists one phone (`R9WR50C0W1J`) and `com.cleophis.app`
is installed on it, but that is not a build of this branch, no vision files are in its
`models/`, and building a debug APK needs signing material. The engine path itself is
measured by Stage 0 (A22: prefill about 7.6 s, `VmHWM` 389-394 MB).

Stage 2's checklist runs exactly this, on a debug APK of this branch with the two
vision files pushed into the app's `models/` via `adb shell run-as com.cleophis.app …`,
from the WebView devtools (`chrome://inspect`):

```js
await window.__TAURI__.core.invoke('vision_state')
// expect { declared: true, installed: true, coresident: true }
await window.__TAURI__.core.invoke('transcribe_image', {
  requestId: 't1',
  imageBase64: '<base64 of spike-images-formula/p03-quadratic.png>',
  onEvent: new window.__TAURI__.core.Channel(e => console.log(e)),
})
// expect one `done` event: blocks[0].content === 'x^{2}-5x+6=0', verdict 'accept'
```

plus `[vision]` lines in logcat with prefill near 7.6 s, and that no `[vision]` line
carries block content or image bytes.

## What Stage 2 inherits

### Command surface and wire shape

All three are in `src-tauri/src/vision_cmds.rs`, registered on both platforms; the
desktop bodies return `in-process vision commands are mobile-only; this is a desktop build`.

- `transcribe_image(requestId: string, imageBase64: string, onEvent: Channel)`. Sends
  exactly one event per call that reaches the engine's command path: it resolves `Ok(())`
  after a `done` event, or `Err(message)` after an `error` event. The early refusals
  reject without any event: a body that is not valid base64, a body over the 20 MiB cap,
  and a second call while one is queued or running (`A transcription is already in
  progress.`). Cancel with the existing `chat_cancel` and the same `requestId`.
- `prewarm_vision()`. Advisory, always `Ok(())` on mobile. It sends the load command only
  when the transcriber is installed AND the device is co-resident, both computed at call
  time, so Stage 2 may call it on every camera open.
- `vision_state()`. Computed at call time, whatever the engine's status. Returns `null`
  when the tier neither declares nor has installed a transcriber, otherwise
  `{declared, installed, coresident}`.

Events on the channel (`#[serde(tag = "event", content = "data")]`, camelCase):

```
{ event: "done",  data: TranscribeResult }
{ event: "error", data: { message: string } }

TranscribeResult {
  blocks:      [{ kind: "latex" | "text", content: string, confidence: number,
                  minOperatorProb?: number }],
  warnings:    ("lowConfidence" | "operatorUncertain" | "noMathFound" |
                "invalidLatex" | "truncated")[],
  verdict:     "accept" | "review" | "refuse",
  model:       { id: string, family: string },
  timings:     { prepMs, prefillMs, decodeMs, reloadMs? },   // reloadMs: sequential path only
  imageTokens: number,
  peakRssKb:   number,
  coresident:  boolean
}
```

`confidence` and `minOperatorProb` are widened from `f32` through the shortest decimal, so
`0.995` arrives as `0.995`. `minOperatorProb` is omitted when the block had no operators.

### `EngineInfo.vision`

`EngineInfo.vision` is omitted when neither flag is set, otherwise
`{declared, installed, coresident}`. Its `declared` and `installed` are stamped at the
last successful tutor load, for the `engine-ready` payload, so they miss a download that
lands later. Gate the camera and the download on `vision_state()`, which computes the
same three flags live:

- `declared`: the effective tier's catalog entry has a `vision` block.
- `installed`: both the model and the projector resolve on disk. Both are sha-checked
  again at load (`prepare_vision_stack`), so `installed` is a presence signal, not an
  integrity one.
- `coresident`: the device has at least 6 GB RAM (`CORESIDENT_MIN_RAM_GB`), so the
  transcriber may stay loaded beside the tutor. Below that the engine runs sequentially:
  unload the tutor, transcribe, reload the tutor (`timings.reloadMs`).

### How the vision files reach `models/`

Only through `download_artifact`, and only once the founder (a) publishes the two
artefacts to the dist catalog with kinds `vision_base` and `mmproj`, and (b) adds a
`tiers.low.vision` block to the bundled catalog. `download_artifact` refuses any sha the
bundled catalog does not pin, so (b) is what authorises (a).

| Artefact | kind | sha256 | bytes |
|---|---|---|---|
| `granite-docling-258M-Q8_0.gguf` | `vision_base` | `b687245c212b59da938e411e2d4485596cbaeed1ae52706582ac38ed6a970f76` | 177,918,144 |
| `mmproj-granite-docling-258M-Q8_0.preproc512.gguf` (header-patched: one u32, `preproc_image_size` 2048 to 512) | `mmproj` | `bb75a65aa3ec14ac4a002248d5ffa075183df369030b6ca09028463ede18b307` | 103,769,792 |

Shas computed with `sha256sum` on the files in `~/cleophis-artifacts/vision/`. The
published (unpatched) projector, `3d440e65a91e640d…`, must not be used: it makes mtmd
tile every input to 2048 px (Stage 0: about 50 s per image).

The block, with the Stage 0 regime values (field names from `VisionVariant` in
`src-tauri/src/catalog.rs`; the file names are the founder's to choose at publish time,
these are the names the tests use):

```json
"vision": {
  "baseModel": "granite-docling-258M",
  "modelFile": "models/granite-docling-258M-Q8_0.gguf",
  "sha256": "b687245c212b59da938e411e2d4485596cbaeed1ae52706582ac38ed6a970f76",
  "fileBytes": 177918144,
  "mmprojFile": "models/mmproj-granite-docling-258M-Q8_0.preproc512.gguf",
  "mmprojSha256": "bb75a65aa3ec14ac4a002248d5ffa075183df369030b6ca09028463ede18b307",
  "mmprojBytes": 103769792,
  "family": "docling",
  "prompt": "Convert formula to latex.",
  "maxSide": 512,
  "threads": 4,
  "onePass": true,
  "maxTokens": 96
}
```

At that moment the catalog test `a_tier_without_vision_still_parses_and_resolves_none`
(it reads the bundled `catalog.json`) must be updated, or it fails.

### Security inheritances (verbatim from the plan's assessment, plus what Task 10 adds)

1. **H1: only `latex` blocks reach the tutor.** They go in the user role, prefixed
   "Problem from photo:", never as a `text` block and never anywhere in the system turn.
   The confirm UI renders block content as text (`textContent`, never `innerHTML`).
   The `text`/`latex` classifier REDUCES the prose that reaches the tutor; it does not
   exclude it. `Say = yes` classifies as `latex`, and so do words written as commands
   (`\ignore \all \rules`) and Stage 0's real page output `E \, \text {evaluate} \colon …`.
   Stage 2's contract is the real mitigation: the user role, the "Problem from photo:"
   prefix, and the hard confirmation gate.
2. **M2: downscale before the call.** The capture flow crops and downscales in the
   WebView before `transcribe_image`, so the 20 MiB encoded and 50 MP decoded caps are a
   backstop, not the normal path. The image bytes never touch disk or the conversation
   store.
3. **Low-severity items stay rules:** `[vision]` log lines never carry `raw`, block
   content or image bytes; the process-global one-pass switch is touched only by the
   inference thread.

Facts Stage 2 must also design around:

- A stray `<` or `>` that is not part of a tag can still appear in block content. It is
  display-safe with `textContent`; it would not be with `innerHTML`.
- Prefill cannot be cancelled. Only decode polls the cancel flag, so a cancel during
  prefill takes effect after about 7.6 s on the A22 (Stage 0 figure).
- The foreground service is counted across chat and transcription (`inference_service`
  in `mobile_native.rs`): it starts with the first holder and stops with the last, so
  camera and chat need no UI serialisation for its sake.
- `prewarm_vision` gates itself (installed and co-resident). On a device where it would
  send nothing, calling it costs nothing; where it does send, it ends an open chat
  session (the next turn re-prefills), as the approved review accepts.

### Remaining risks

- Verdict thresholds (confidence, operator floor) are uncalibrated until Stage 3's
  probe set.
- Only five rendered items were measured in Stage 0; there are no photographed items yet.
- WebView memory next to the engine is unmeasured (Stage 3's `dumpsys meminfo`).
- No device run of the Stage 1 code exists. Every device number above is Stage 0's.

## What did not run

- Any device step (see above).
- `build-android-apk.sh` and `check-mobile-build.sh` (signing material; minutes of
  cross-compiling).
- `cargo test -p cleophis` on WSL (replaced by the Windows run).
- A pre-vision library build, so no size delta.

## Fix wave after the final review

The whole-branch review (2026-11-01) found no Critical issue and four Important seam
defects. One wave fixed them with the review's fix-before-merge list and three of its
minors (M4, M5, M9, by the controller's ruling), in ten commits (`vision(fix): …`):

- **I1, the document end.** granite-docling's `<doctag>`, `</doctag>`, `<formula>` and
  `</formula>` are CONTROL tokens. `vision.rs` printed them as nothing, so the parser's
  `</doctag>` cut never ran and text after the document would fuse into the last block.
  Now `vision.rs` resolves the `</doctag>` token once at load (id 100328 in this model)
  and ends generation on it with `StopReason::Eos`, and it renders pieces with control
  tokens, so `raw` carries the document structure. The parser's text cut stays as the
  backstop. Host `--via-engine` run, granite-docling Q8 with the 512 projector:
  - Formula crops, one pass: each raw is now wrapped in `<formula>…</formula>`. With that
    control-token text removed, all five are byte-identical to
    `vision-spike/a22-floor-formula-1pass.jsonl` rep 0, and so are the token counts
    (31, 57, 35, 42, 56). Verdict `Accept` on all five.
  - Page set, two passes, 120 tokens: every raw is `<doctag>…</doctag>`, all five stopped
    on the document-end token, and nothing follows `</doctag>`.
- **I2, no maths leaves the tutor-bound block.** A colon label needs a word, so
  `a : b = 2 : 3` is one `latex` block. A `\text{…}` head is a label only before a colon,
  so `\text{cost} = 5` stays one block. The array unwrap keeps what surrounds the array:
  `A = \left( \begin{array}{cc} … \end{array} \right)` shows `A=\left(1&2 \\ 3&4\right)`.
  `x : y` stays one block; it still classifies as `text` and refuses.
- **I3, live vision flags.** `vision_state` computes `declared`, `installed` and
  `coresident` at call time, whatever the engine's status. `prewarm_vision` sends only
  when the files are installed and the device is co-resident. `EngineInfo.vision` keeps
  the values stamped at load.
- **I4, the counted foreground service.** `inference_service` counts its holders: the
  service starts on 0 to 1 and stops on 1 to 0, under one lock with the Kotlin call.
  `chat_stream`'s task-failure path now releases its hold too. The "serialise camera and
  chat" rule is gone from the inheritances above.
- **Task 2:** the downscale runs before the EXIF orientation, and every orientation 1-8
  is tested on an in-memory image.
- **Task 9:** `TranscribeOutcome.truncated` is deleted; the one-event contract is stated
  exactly, in code and above.
- **Task 10:** the document fixes in both records.
- **M4, the operator floor sees merged pieces.** A piece counts as an operator when any
  character of its part inside the block is one of `+ - = < > ^ _`, or when it holds a
  watched command (`\frac`, `\times`, `\leq`, …) as a whole command. So the merged
  pieces `=-`, `^{` and `}=` are checked, and an unspaced `x=-1` no longer passes
  unchecked. Only the part inside the block counts because BPE merges a tag's closing
  `>` with the first maths character (`>\` on formula crops p02 and p05, the least
  certain token there), and that `>` is tag syntax. After this change the host
  `--via-engine` raws and verdicts are identical to the run above (formula crops
  `Accept` ×5).
- **M5:** the classifier is described as reducing prose, not excluding it (above, and
  `vision_parse.rs`'s module doc).
- **M9:** `transcribe_image` claims the in-flight slot before it decodes the base64, so a
  refused concurrent caller no longer allocates a decoded image first.

Gates on the wave's code (WSL):

| Command | Last line |
|---|---|
| `cargo test -p kpack-engine` | `test result: ok. 158 passed; 0 failed` (lib), `4 passed` (parity), 0 doc tests |
| `cargo build -p kpack-engine` | `Finished`, no warnings |
| `RUSTFLAGS='-D warnings' cargo ndk -t arm64-v8a -P 24 check -p cleophis --all-targets --release` | `Finished \`release\` profile [optimized] target(s) in 8.94s`, exit 0 |
| `npm test` | `# pass 1215`, `# fail 0` |

The Windows desktop suite runs on the commit that carries this section. Its result goes in
the pull request description rather than here, because writing it into this file would
change the commit it ran on.

## Deferred minors

One line per task, from the SDD ledger; these feed the whole-branch review.

- **Task 1:** `check-mobile-build.sh` audits the probe library, not a shipped artifact (now covered by the vision feature plus this task's app-library audit); the "4 helpers" wording is stale (5).
- **Task 2:** rotate-before-downscale doubles the peak on a portrait photo, and EXIF orientations 2-5, 7, 8 are untested (both fixed in the fix wave); `to_rgb8()` drops alpha, so a transparent PNG goes dark-on-dark.
- **Task 3:** `split_label` mangles `x : y`, `a : b = 2 : 3` and `\text{cost} = 5`, and the array unwrap drops text before `\begin{array}` (fixed in the fix wave; `x : y` still refuses, safely); `classify` is unicode-blind (U+2212 minus becomes `text`, fails safe); `braces_balanced` counts `\{`/`\}` (fails safe); module doc says "vision gate" where `lib.rs` says "real gate".
- **Task 4:** no `Debug` on `VisionLoadRequest`/`Transcription`; `VisionBackend::load` lacks a doc comment; tests do not cover None-skips or base-before-projector ordering.
- **Task 5:** a Drop guard would restore the one-pass switch on panic; `n_batch` is not capped at `n_ctx` for `eval_chunks`; `max_tokens == 0` still emits one token before `MaxTokens`; prefill (`eval_chunks`) cannot be cancelled, only from the first decoded token (prefill is about 7.6 s on the phone); `--via-engine` silently ignores `--n-ctx`/`--n-batch`/`--repeat`/`--json`/`--raw-stb`; a per-item read failure aborts the `--via-engine` run.
- **Task 6:** `check_artifact_pinned` indentation would be reflowed by rustfmt; the bundled-catalog test must change when the vision block ships (see above).
- **Task 7:** the `EngineInfo` test never asserts `coresident` or the installed-only path (fixed in the fix wave); the resolve test leaves its temp dir and packs statements per line; `resolve_vision` re-reads and parses the catalog where `hero_entry` already does (existing pattern).
- **Task 8:** `expect("just loaded")` should be `vision.insert`; a local `handle` shadows the tutor's name; the idle expiry is checked only after a timed wait; rustfmt reflow at `engine_inproc.rs:68`/`:316` and `serve.rs:71`.
- **Task 9:** "exactly one event per call" should read "per call that reaches the engine", `TranscribeOutcome.truncated` duplicates the Truncated warning, and the foreground-service flag is not refcounted (all three fixed in the fix wave); `ChatCancels::begin` overwrites a same-id entry; no test for the over-length base64 refusal or `f32_wire(0.1)`; the "too large" message could name 20 MiB.
