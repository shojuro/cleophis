# behavioral-v1 training recipe (recovered)

Recovered 2026-07-21 from the surviving adapter artifacts in the private B2 bucket
`cleophis-models`, since the original training Colab is gone. Source objects:

- `adapters/behavioral/v1/Qwen3-4B-unsloth-bnb-4bit/adapter_config.json`
- `adapters/behavioral/v1/Qwen3-4B-unsloth-bnb-4bit/manifest.json`
- `adapters/behavioral/v1/Qwen3-4B-unsloth-bnb-4bit/README.md`
- Cross-checked against the sibling `Llama-3.2-1B-Instruct-bnb-4bit` and
  `Qwen3-8B-unsloth-bnb-4bit` adapter dirs — the LoRA config below is identical
  across all three existing tiers (only `base_model_name_or_path` differs).

## LoRA config (recovered — verbatim from `adapter_config.json`)

| field | value |
|---|---|
| `r` | 16 |
| `lora_alpha` | 16 |
| `lora_dropout` | 0 |
| `bias` | `none` |
| `target_modules` | `q_proj`, `k_proj`, `v_proj`, `o_proj`, `gate_proj`, `up_proj`, `down_proj` (all attention + MLP projections) |
| `use_rslora` | `false` |
| `use_dora` | `false` |
| `task_type` | `CAUSAL_LM` |
| base 4-bit repo (Qwen3-4B tier) | `unsloth/Qwen3-4B-unsloth-bnb-4bit` |
| `peft_version` | 0.19.1 |

## Training hyperparameters — MISSING

**Learning rate, epochs/max_steps, per-device batch size, gradient accumulation
steps, training max_seq_len, and random seed are not recoverable.** They do not
appear in any surviving artifact:

- `manifest.json` only carries provenance metadata: `base_model`,
  `dataset_version`, `contract_version`, `provider`/`gen_model`,
  `trained_at_utc`, and per-file/per-dataset sha256 hashes. No training
  hyperparameters.
- `README.md` is the unmodified PEFT/unsloth auto-template — the "Training
  Hyperparameters" section still reads `[More Information Needed]`; it was
  never filled in.
- No `trainer_state.json`, `training_args.bin`/`.json`, or notebook survives.
  The full bucket listing for each adapter dir contains only: `README.md`,
  `adapter_config.json`, `adapter_model.safetensors`, `chat_template.jinja`,
  `manifest.json`, `tokenizer.json`, `tokenizer_config.json`.

Confirmed **MISSING** (flag for Task 2 to fall back to behavioral-v1-consistent
defaults, not fabricate a specific prior value):
- `learning_rate`
- `num_train_epochs` / `max_steps`
- `per_device_train_batch_size`
- `gradient_accumulation_steps`
- max_seq_len (training-time)
- `seed`

Note: `tokenizer_config.json` for the Qwen3-4B adapter has `model_max_length:
40960` — this is Qwen3's own default tokenizer context window, **not** a
recorded training `max_seq_len`. Do not treat it as authoritative for the
training config.

## Dataset

| field | value |
|---|---|
| train path | `datasets/behavioral/v1/final_train.jsonl` (18,593,749 bytes / ~18.6 MB) |
| eval path | `datasets/behavioral/v1/final_eval.jsonl` (371,619 bytes / ~372 KB) |
| `dataset_version` (manifest) | `behavioral-v1` |
| `contract_version` (manifest) | `prompt-contract-v0` |
| `provider` / `gen_model` (manifest) | `deepseek` / `deepseek-chat` |

Format: JSONL, one JSON object per line, OpenAI-style chat schema:

```json
{"messages": [{"role": "system", "content": "..."}, {"role": "user", "content": "..."}, {"role": "assistant", "content": "..."}, "..."], "source": "synthetic_deepseek", "category": "educational_socratic"}
```

- `messages`: multi-turn system/user/assistant conversation.
- `source`: generation provenance tag (e.g. `synthetic_deepseek`).
- `category`: behavioral category tag (e.g. `educational_socratic`).

The `datasets` hash block in `manifest.json` (sha256 per file) is byte-identical
across all three existing tier adapters — confirming a single shared dataset
was used to train every tier, not a per-tier split.

## New base for the Qwen3-1.7B tier

Confirmed via Hugging Face MCP `hub_repo_details`:

- **`unsloth/Qwen3-1.7B-unsloth-bnb-4bit`** — license `apache-2.0`, ungated,
  4-bit bnb-quantized, ~1.76B params, architecture `qwen3`.
- Matches the naming convention of the other two tiers'
  `base_model_name_or_path` (`unsloth/<Model>-unsloth-bnb-4bit`), so Task 2
  should use this repo id as the LoRA base.
- The non-quantized `unsloth/Qwen3-1.7B` also exists if a non-4-bit base is
  ever needed, but the 4-bit variant is what's consistent with this recipe.

## Summary for Task 2

Use the LoRA config table above verbatim (`r=16`, `lora_alpha=16`,
`lora_dropout=0`, `bias=none`, the 7-module `target_modules` set,
`use_rslora=false`, `use_dora=false`) against
`unsloth/Qwen3-1.7B-unsloth-bnb-4bit`, trained on
`datasets/behavioral/v1/final_train.jsonl` /
`datasets/behavioral/v1/final_eval.jsonl`. Training hyperparameters
(lr/epochs/batch/grad-accum/max_seq_len/seed) are not recoverable — pick
behavioral-v1-consistent defaults explicitly, don't assume any specific prior
value.
