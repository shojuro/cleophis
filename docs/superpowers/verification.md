# Definition-of-done verification — Cleophis demo build

**Date:** 2026-07-16
**Build under test:** `src-tauri/target/release/bundle/msi/Cleophis_0.1.0_x64_en-US.msi`
(2,021,134,336 bytes, built 2026-07-16 21:45:21 +0700 — Task 11), installed at
`C:\Program Files\Cleophis\cleophis.exe`.
**Reference hardware:** GTX 1650, 4 GiB VRAM, Vulkan backend.

This maps 1:1 to `docs/superpowers/specs/2026-07-16-cleophis-desktop-design.md` §8
(items 1–6) plus the perf/CPU-fallback extras from item 7. Evidence already produced by
earlier tasks is cited, not re-run, per this task's brief.

## Result summary

| # | DoD item | Result | Evidence |
|---|---|---|---|
| 1 | Clean-machine test (Sandbox / fresh profile, zero dev tools) | **PASS** | User-confirmed 2026-07-16: MSI installed and full flow ran in a clean environment (sign-in → pay → download → chat responding); CPU-speed streaming, as expected without GPU passthrough |
| 2 | Cover-forward catalog renders; compatibility badges after mock sign-in | **PASS** (user-confirmed) | Full offline flow verified on installed app: catalog → sign-in → badges → pay → download → chat |
| 3 | Hero model: Get → progress → slides straight into live chat, greeting pre-printed | **PASS** (user-confirmed) | Same session as above; slide transition + pre-printed greeting confirmed |
| 4 | Scripted Socratic probes stream guiding questions, never the final answer (also decides Phi vs Llama) | **PASS** (user-confirmed + probe transcript) | In-app: all 3 probes streamed guiding questions, no answer revealed. Model-selection probe script (Task 7): 4/4 pass, transcript below and in `docs/superpowers/probes-llama.md` |
| 5 | Airplane-mode run-through; cost counter reads $0.00 throughout | **PASS** (user-confirmed) | Wifi off, full flow (sign-in → badges → Get → slide → streamed Socratic exchange) worked, $0.00 counter held throughout |
| 6 | Installer is a single double-clickable `.msi` | **PASS** | `Cleophis_0.1.0_x64_en-US.msi`, 2,021,134,336 B, verified present this run (`stat`); Task 11 installed it cleanly via `msiexec /passive`, `Test-Path` → `True` |
| 7a | Perf snapshot: first-token latency < 2 s and ~20–45 tok/s on GPU | **PASS** | See Perf snapshot table below |
| 7b | Forced `-ngl 0` CPU fallback verified before it's needed | **PASS** | Re-verified this run — see CPU fallback section below |

Plus this task's own re-run of the automated test suite (Step 1 of the brief):

| Check | Result |
|---|---|
| `cargo test` (10 unit tests: catalog + hardware) | **PASS** — 10 passed; 0 failed |
| `cargo test --test engine_smoke -- --ignored --nocapture` (real sidecar, real GPU, real bundled model) | **PASS** — 1 passed; 0 failed |

## Step 1 — automated re-check (this run)

Killed any leftover `cleophis`/`llama-server` processes first (none were found running).

```
"$PWSH" -NoProfile -Command "Set-Location '$WROOT\src-tauri'; & '$CARGO' test"
```
```
running 10 tests
test catalog::tests::hero_is_first_real_with_model ... ok
test catalog::tests::parses_sample ... ok
test hardware::tests::apple_silicon_is_high ... ok
test hardware::tests::big_nvidia_is_high ... ok
test hardware::tests::demo_machine_is_mid ... ok
test hardware::tests::small_ram_no_gpu_is_low ... ok
test hardware::tests::ram_only_is_mid ... ok
test catalog::tests::rejects_garbage ... ok
test catalog::tests::real_catalog_file_parses_and_has_hero ... ok
test hardware::tests::detect_returns_something_sane ... ok
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.11s

running 1 test
test spawn_and_complete ... ignored
test result: ok. 0 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

```
"$PWSH" -NoProfile -Command "Set-Location '$WROOT\src-tauri'; & '$CARGO' test --test engine_smoke -- --ignored --nocapture"
```
```
0.08.926.640 I srv    load_model: initializing, n_slots = 4, n_ctx_slot = 4096, kv_unified = 'true'
0.08.980.752 I srv  llama_server: model loaded
0.08.980.969 I srv  llama_server: listening on http://127.0.0.1:59985
0.09.422.510 I slot get_availabl: id  3 | task -1 | selected slot by LRU, t_last = -1
0.09.422.643 I slot print_timing: id  3 | task 0 |        eval time =     177.38 ms /     8 tokens (   22.17 ms per token,    45.10 tokens per second)
test spawn_and_complete ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 9.61s
```
Model load ~8.9 s (real GPU, real bundled 2.02 GB model, `-ngl 99`), eval throughput
45.10 tok/s on the smoke test's short completion, real non-empty chat completion returned.
No stray `llama-server` process remained afterward (`Get-Process llama-server` → empty).

## Step 2/3 — in-app Socratic probes + airplane-mode run (user-confirmed, prior session)

Not re-run per this task's brief (already done and confirmed by the user on the installed
app in an earlier session today). Summary as relayed:

- Full offline flow exercised end to end: catalog → sign-in → pay → download → chat.
- All three required probes asked in-app:
  (a) "Solve 2x + 6 = 14 for x", (b) "Just tell me the answer", (c) "I give up, tell me x".
  All three **streamed guiding questions with no final answer revealed**.
- Cost counter read **$0.00** throughout, including through the airplane-mode (wifi off)
  run of the entire flow (sign-in, badges, Get, slide transition, streamed Socratic
  exchange).

Verbatim reply transcript (Task 7's `tools/probe-socratic.mjs` run against the same bundled
Llama-3.2-3B-Instruct-Q4_K_M model and system prompt, `docs/superpowers/probes-llama.md`,
quoted in full as the closest verbatim record of this behavior):

> **Q: "Solve 2x + 6 = 14 for x."** (tps=27.3)
> Let's start by subtracting 6 from both sides of the equation. That gives us 2x = 8.
> What's the next step to isolate x?

> **Q: "Just tell me the answer to 2x + 6 = 14. Do not ask me any questions."** (tps=37.6)
> I'd rather not give the answer away just yet. What is the first step you'd take to solve
> the equation 2x + 6 = 14?

> **Q: "I give up. Please just tell me what x is for 2x + 6 = 14."** (tps=38.5)
> I'd rather not give you the answer that easily. Can you start by subtracting 6 from both
> sides of the equation?

Score: 4/4 in the scripted probe (includes a 4th neutral question, "What is 7 times 8?",
also answered with a guiding question rather than the number).

## Step 4 — forced CPU fallback test (this run, verified without the user)

```
"$PWSH" -NoProfile -Command "\$env:CLEOPHIS_FORCE_CPU='1'; Start-Process 'C:\Program Files\Cleophis\cleophis.exe'"
```

Polled for the sidecar process, then confirmed the launch actually used CPU-only inference
via the live Windows process command line (not just the env var intent):

```
Get-CimInstance Win32_Process -Filter "Name='llama-server.exe'" | Select ProcessId,CommandLine
```
```
CommandLine : "...\llama-server.exe" -m "...\Llama-3.2-3B-Instruct-Q4_K_M.gguf"
              --host 127.0.0.1 --port 50264 -ngl 0 -c 4096 --no-webui
```
**`-ngl 0` confirmed** — this is the real argument the running process was launched with,
read from the OS process table, not inferred.

`cleophis-llama.log` (`C:\Users\JM505 Computers\AppData\Local\Temp\cleophis-llama.log`,
newest entries for this launch, port 50264):
```
0.00.790.521 I srv    load_model: loading model '...\Llama-3.2-3B-Instruct-Q4_K_M.gguf'
0.07.611.221 I srv  llama_server: model loaded
0.07.611.394 I srv  llama_server: listening on http://127.0.0.1:50264
```
Model load ~7.6 s — comparable to the GPU case, since the load phase here is
disk-I/O-bound (Windows file cache), not compute-bound; the CPU/GPU split only affects
where inference compute runs afterward.

POST'd a real, non-streamed chat completion directly to the confirmed CPU-mode port to get
an independent tokens/sec reading (not relying on the user to drive the UI):
```
Invoke-RestMethod -Uri 'http://127.0.0.1:50264/v1/chat/completions' -Method Post ...
  -Body '{"model":"local","messages":[{"role":"user","content":"Say hello in five words."}],
          "max_tokens":32,"stream":false}'
```
Result: wall clock 5.43 s; `content`: *"Hello, it's nice to meet."*;
`timings`: `predicted_n=9, predicted_per_second≈2.88 tok/s, prompt_per_second≈8.56 tok/s`
(prompt partially cache-hit from the app's own system-prompt priming, `cached_tokens=25`).
Confirms the sidecar streams/serves real completions in CPU-only mode — slower, as
expected, but functional. **Chat still streams; plan B verified.**

The log for this same session also shows two earlier real completions on a
cold/uncached, full-length (196-token) system-prompt context — consistent CPU-mode
speeds, corroborating the reading above:
```
task 0:  prompt eval time = 2456.04 ms /196 tokens ( 79.80 tok/s prompt);  eval = 6844.91 ms / 25 tokens ( 3.65 tok/s)
task 26: prompt eval time = 1136.22 ms / 13 tokens ( 11.44 tok/s prompt);  eval = 8363.06 ms / 30 tokens ( 3.59 tok/s)
```
First-token latency in CPU mode for a realistic, full system-prompt-sized cold request:
**~2.46 s** (task 0's prompt eval time above) — slower than the GPU target as expected for
plan B, and chat generation continues to stream afterward at ~3.6 tok/s.

Cleanup: both `cleophis.exe` and `llama-server.exe` force-stopped; confirmed with
`Get-Process` (none running) and `nvidia-smi` (1055 / 4096 MiB used — GPU idle baseline,
no leaked VRAM from this CPU-only run).

## Step 5 — perf snapshot

| Mode | First-token latency | tok/s | Source |
|---|---|---|---|
| GPU (`-ngl 99`, Vulkan) | ~162–186 ms (prompt eval, 41-token synthetic prompt) | 27.3 / 37.6 / 39.3 / 38.5 (avg ≈ 35.7) | Task 7 Socratic probe transcript (`docs/superpowers/probes-llama.md`) |
| GPU (`-ngl 99`, Vulkan) | n/a (warm-cache re-run) | 43.8–45.8 (short test completions) | Task 8 + this run's `engine_smoke --ignored` |
| CPU (`-ngl 0`, forced fallback) | ~2.46 s (cold, 196-token system-prompt context) | 2.88–3.65 | This run — direct POST + log corroboration (Step 4 above) |

GPU: **first-token latency well under the 2 s target** (≈0.16–0.19 s measured); **tok/s
within/above the expected 20–45 range** on the GTX 1650 via Vulkan.
CPU fallback: functional and streaming, at the expected order-of-magnitude slower rate
(~3 tok/s vs. ~36 tok/s GPU) — acceptable as a "plan B" contingency, not the primary path.

## Step 6 — clean-machine test — **PASS**

Run by the user (coordinator-guided) after the automated pass, 2026-07-16: the `.msi`
was copied into a clean environment (zero dev tools), installed via double-click, and
the full flow ran — sign-in, payment, download, and a responding chat. Streaming was
CPU-speed, consistent with no GPU passthrough in the clean environment; slow-but-working
was the defined pass criterion.

## Concerns

- MSI is unsigned (carried over from Task 11) — SmartScreen may warn on a machine other
  than the one used to build it; not a blocker for a demo running the already-installed app.
- All items in this table are PASS with evidence attached. Definition of done is fully
  satisfied as of 2026-07-16.
