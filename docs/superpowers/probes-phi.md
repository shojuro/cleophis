# Probe transcript — phi

**Result: DID NOT LOAD (0/4 — server crashed before any probe could run)**

`llama-server.exe` was started twice, identically to the Llama run, per the brief:

```
"$PWSH" -NoProfile -Command "& '$WROOT\src-tauri\resources\llama\llama-server.exe' -m '$WROOT\models-lab\Phi-3.5-mini-instruct-Q4_K_M.gguf' --host 127.0.0.1 --port 8089 -ngl 99 -c 4096 --no-webui"
```

Both attempts failed identically with a Vulkan out-of-device-memory error while allocating
compute buffers, before the HTTP server ever came up (health check never returned 200; no
probe request could be sent). `nvidia-smi` confirmed no leftover `llama-server` process and
~3.1 GiB of the GTX 1650's 4 GiB VRAM free before each attempt, so this is not a leaked-memory
artifact from the prior Llama run — it is Phi-3.5-mini-instruct-Q4_K_M (3.8B params, 2,393,232,672 B
file) genuinely not fitting fully offloaded (`-ngl 99`) with a 4096-token context on 4 GB of VRAM.

Attempt 1 and attempt 2 log tail (identical):

```
0.00.5xx W common_fit_params: failed to fit params to free device memory: n_gpu_layers already set by user to 99, abort
0.00.8xx W load_arch_hparams: Phi SWA is currently disabled - results might be suboptimal for some models (see https://github.com/ggml-org/llama.cpp/pull/13676)
0.00.8xx W load: control-looking token:      2 '</s>' was not control-type; this is probably a bug in the model. its type will be overridden
ggml_vulkan: Device memory allocation of size 67119120 failed.
ggml_vulkan: vk::Device::allocateMemory: ErrorOutOfDeviceMemory
0.05-0.09s E ggml_gallocr_reserve_n_impl: failed to allocate Vulkan0 buffer of size 67119120
0.05-0.09s E graph_reserve: failed to allocate compute buffers
0.05-0.09s E llama_init_from_model: failed to initialize the context: failed to allocate compute pp buffers
0.05-0.09s E cmn  common_init_: failed to create context with model '...\models-lab\Phi-3.5-mini-instruct-Q4_K_M.gguf'
```

Since Phi-3.5-mini-instruct-Q4_K_M cannot be loaded fully-offloaded on the reference GPU at all
(a hard blocker independent of the 2 GiB packaging cap it also exceeds), the brief's decision
rule (higher pass count wins) resolves immediately in Llama's favor without needing to test the
IQ4_XS fallback: Llama already scored a perfect 4/4 and actually runs. No behavioral probe data
exists for Phi-3.5-mini-instruct-Q4_K_M because the server never became healthy.
