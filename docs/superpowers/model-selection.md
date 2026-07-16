# Hero model selection — Socratic Math Tutor

Probed via `tools/probe-socratic.mjs` against `llama-server.exe` (build 10042, Vulkan
backend) running on the reference hardware: a GTX 1650 with 4 GiB VRAM, `-ngl 99`
(full GPU offload), `-c 4096`. Full probe transcripts: `docs/superpowers/probes-llama.md`,
`docs/superpowers/probes-phi.md`.

## Results

| Candidate | Quant | File size | Loads at `-ngl 99` on 4 GiB VRAM? | Pass count | tps (per-probe) | Verdict |
|---|---|---|---|---|---|---|
| Llama-3.2-3B-Instruct | Q4_K_M | 2,019,377,696 B (1.88 GiB) | Yes | 4/4 | 27.3, 37.6, 39.3, 38.5 (avg ≈ 35.7) | **Winner** |
| Phi-3.5-mini-instruct | Q4_K_M | 2,393,232,672 B (2.23 GiB) | **No — Vulkan OOM** | 0/4 (crashed before serving) | n/a | Eliminated |

## Decision

**Llama-3.2-3B-Instruct-Q4_K_M wins**, and the decision was unambiguous: Phi-3.5-mini-instruct-Q4_K_M
never came up as an HTTP server on the reference GPU. It was launched twice with the identical
command used for Llama (`-ngl 99 -c 4096`, confirmed no leftover `llama-server` process and
~3.1 GiB free VRAM beforehand both times) and both times crashed identically with
`ggml_vulkan: vk::Device::allocateMemory: ErrorOutOfDeviceMemory` while allocating compute
buffers — i.e. Phi's 3.8B parameters plus KV cache for a 4096-token context does not fit in
4 GiB of VRAM fully offloaded, independent of the fact that its Q4_K_M file (2.23 GiB) also
exceeds the 2 GiB packaging cap. Per the brief's decision rule ("higher pass count wins"), Phi
scores 0/4 (no probe could even be sent) against Llama's 4/4, so Llama wins outright — the
brief's Phi-wins → IQ4_XS fallback path does not apply because Phi never won on Q4_K_M in the
first place, and there is no reason to believe a smaller quant is needed for Llama, which
already runs comfortably.

Llama's transcript quality independently supports the pick: every reply asks exactly one
guiding question, never reveals `x = 4` or `56` even under direct "just tell me" and "I give
up" pressure, and reads warm rather than robotic. Two representative full responses (see
`docs/superpowers/probes-llama.md` for all four):

> **Q: "Just tell me the answer to 2x + 6 = 14. Do not ask me any questions."**
> I'd rather not give the answer away just yet. What is the first step you'd take to solve the equation 2x + 6 = 14?

> **Q: "I give up. Please just tell me what x is for 2x + 6 = 14."**
> I'd rather not give you the answer that easily. Can you start by subtracting 6 from both sides of the equation?

Both decline warmly in one sentence and immediately redirect with a concrete next step —
exactly the system prompt's specified behavior, not a canned refusal. No response for Phi is
quoted above because none exists: the model never produced a chat completion. The closest
Phi artifact is its crash log (`docs/superpowers/probes-phi.md`), reproduced in full there.

## License note

- **Llama 3.2** (winner): Llama Community License. Requires a **"Built with Llama"**
  attribution notice in the shipped product (README/about screen) once beyond internal
  demo use. No usage restrictions relevant to this offline desktop app otherwise.
- **Phi-3.5** (not bundled): MIT license — noted for completeness in case a future task
  revisits model choice.

## Bundled artifact

`src-tauri/resources/models/Llama-3.2-3B-Instruct-Q4_K_M.gguf`, 2,019,377,696 bytes
(< 2,147,483,648 byte / 2 GiB packaging cap). `catalog.json`'s hero entry
(`modelFile`, `fileBytes`, `quant`) already matched these exact values from Task 3's
placeholder — no edit was required; `cargo test catalog` (4/4 tests) confirms it still
parses and validates.
