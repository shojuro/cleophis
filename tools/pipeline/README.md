# Cleophis distribution pipeline (`tools/pipeline/`)

This is Track A: the producer side of model distribution. It builds the
base-model and adapter GGUFs, assembles + signs `catalog.json`, uploads
everything to the public `cleophis-dist` bucket, and self-checks the
result. Track B (the app, already shipped) fetches the signed catalog and
downloads artifacts by sha256 — this directory is the only thing that
writes to that bucket.

It runs as plain `python3`/`bash` in WSL2 / Linux — **not** the Windows
PWSH toolchain the Rust crates use. Nothing here needs Windows.

## Status

Task A1 (this commit) is scaffold + tooling only: `setup_tools.sh`,
`requirements.txt`, `.env.example`, and this README. The actual build/sign/
publish scripts (`build_base.py`, `build_adapter.py`, `build_catalog.py`,
`sign_catalog.py`, `generate_curator_key.py`, `publish.py`,
`verify_published.py`) land in tasks A2–A6 and are documented here, script
by script, as each one lands.

## Setup

```bash
cd "tools/pipeline"   # quote the path — the repo root has a space in it
./setup_tools.sh
cp .env.example .env  # then fill in real values (see below)
```

`setup_tools.sh` is idempotent — safe to re-run any time; it skips
whatever's already present and only fixes/fetches what's missing. It needs
no credentials: everything it does is cloning/downloading public
llama.cpp release artifacts and building a local Python venv.

It sets up three things:

1. **A pinned llama.cpp checkout** at `tools/pipeline/llama.cpp/` — shallow
   git clone of `ggml-org/llama.cpp`, for `convert_hf_to_gguf.py` and
   `convert_lora_to_gguf.py` (the base-model / adapter GGUF converters).
2. **A `llama-quantize` binary** at `tools/pipeline/llama.cpp/bin/
   llama-quantize` — downloaded prebuilt from the same tag's GitHub release
   (ubuntu-x64 asset, sha256-verified against a pin in the script), falling
   back to a local `cmake` source build only if no usable prebuilt binary
   can be obtained or it doesn't actually execute in this environment.
3. **A Python venv** at `tools/pipeline/.venv/` — this pipeline's own
   dependencies (`requirements.txt`: `huggingface_hub`, `boto3`,
   `cryptography`) plus the convert scripts' own dependencies, installed
   from llama.cpp's `requirements/requirements-convert_lora_to_gguf.txt`
   (which pulls in `requirements-convert_hf_to_gguf.txt` and
   `requirements-convert_legacy_llama.txt` transitively — one `pip install`
   covers both convert scripts). That pulls a CPU build of torch, a
   multi-GB download; expect the first run to take a while.

After it finishes, later scripts invoke tools directly — no `PATH` or
`LD_LIBRARY_PATH` setup needed:

- `tools/pipeline/.venv/bin/python3 tools/pipeline/llama.cpp/convert_hf_to_gguf.py ...`
- `tools/pipeline/.venv/bin/python3 tools/pipeline/llama.cpp/convert_lora_to_gguf.py ...`
- `tools/pipeline/llama.cpp/bin/llama-quantize ...` (the prebuilt binary's
  `RUNPATH` is `$ORIGIN`, so it finds its sibling `.so` libraries
  regardless of caller `cwd`/`LD_LIBRARY_PATH`)

### Pinned llama.cpp version

| | |
|---|---|
| Tag | `b10042` |
| Resolved commit | `3f08ef2c519710831cb68c8dc2c2693e6bb5bf81` |
| Prebuilt asset | `llama-b10042-bin-ubuntu-x64.tar.gz` |
| Asset sha256 | `132d5c09e1d8087bb68ddc7876e69e7e82ae503493933f5706163b10b7036eee` |

This tag matches the llama-server release the app itself bundles (see
`../fetch-llama-server.mjs`'s release-asset fetch — that script tracks
"latest" for its Windows Vulkan build; this pipeline pins the exact same
tag deliberately, so converted/quantized artifacts match the runtime
llama.cpp build). `setup_tools.sh` hardcodes this tag, commit, and asset
sha256 and verifies against them on every run — bumping the pin means
updating all three constants at the top of `setup_tools.sh` (and this
table) together, deliberately, not an automatic "latest" fetch.

## The env-dumb contract

Every script in this directory is **env-dumb**: its only inputs are

- CLI arguments (explicit, no hidden defaults that depend on who's running
  it or from where),
- `tools/pipeline/.env` (via `python-dotenv`-style loading or equivalent —
  see `.env.example` for the full var list), and
- `tools/pipeline/.venv` + the pinned `llama.cpp/` checkout that
  `setup_tools.sh` sets up.

No script assumes anything about the calling shell's environment, working
directory beyond its own `tools/pipeline/` root, or machine-specific state.
Each script documents, in its own header comment once it lands (A2–A6):

- **Inputs** — exact CLI args / env vars it reads.
- **Outputs** — exact file(s)/path(s) it writes, under `tools/pipeline/
  work/` (gitignored scratch) unless it's the final publish step.
- **Hashes** — every artifact it produces or uploads is paired with its
  sha256, computed by the script itself (never trusted from an external
  source) so the same hash a script logs is the one that ends up in
  `catalog.json`.

`.env` itself is gitignored (`.env.example` here is the committed
template) and read only by scripts that need bucket/HF credentials. See
`.env.example`'s comments for what each variable is for and, critically,
**`CURATOR_KEY_FILE` is a path, not a secret value** — it must point
OUTSIDE this repo, and only the `sign_catalog.py` step (A4) ever reads the
file at that path. The private key material itself never goes in `.env`
and is never committed.

## Binding cross-track values

These values are fixed by the spec and by Track B's already-shipped Rust
code (`crates/kpack-core/src/sign.rs`, `src-tauri/src/catalog_dist.rs`).
They are recorded here **verbatim** so later pipeline tasks (A2–A6) can't
drift from what the app actually expects — treat every value in this
section as load-bearing, not a suggestion:

- **Base artifact basename:** `Qwen3-4B-Instruct-Q4_K_M.gguf`
- **Adapter artifact basename:** `behavioral-v1-Qwen3-4B.gguf`
- **`base_model` field:** exactly `"Qwen3-4B"` (case-sensitive) for every
  artifact in `catalog.json`, base or adapter.
- **`kind` field:** `"base"` for the base model artifact, `"adapter"` for
  the LoRA adapter artifact — no other values.
- **`catalog.json.sig`:** a RAW 64-byte binary detached ed25519 signature
  over the exact bytes of `catalog.json` — never base64, never hex text,
  never any other encoding. `sign_catalog.py` must write exactly 64 raw
  bytes to this file. This is what
  `kpack_core::sign::verify_detached`/`verify_file` on the app side reads
  back (see `crates/kpack-core/src/sign.rs`'s `read_sig_capped`, which
  rejects anything other than exactly 64 bytes) — reusing that one crypto
  path, no new signature format.
- **Catalog wire format** (snake_case; must match
  `src-tauri/src/catalog_dist.rs::DistCatalog`/`Artifact` field-for-field,
  since that struct derives `Deserialize` with no `rename` — any mismatch
  is a hard parse failure on the app side, not a silent default):

  ```json
  {
    "catalog_version": 1,
    "generated_at": "2026-07-21T00:00:00Z",
    "artifacts": [
      {
        "path": "models/Qwen3-4B/v1/Qwen3-4B-Instruct-Q4_K_M.gguf",
        "sha256": "<hex>",
        "size": 0,
        "kind": "base",
        "base_model": "Qwen3-4B",
        "version": "v1",
        "license": "<license identifier>"
      }
    ]
  }
  ```

- **Bucket layout:** artifacts live at **immutable, versioned paths**
  under `cleophis-dist` (a new build is a new path — never overwrite an
  existing artifact path). The only files ever replaced in place are
  `catalog.json` and `catalog.json.sig`, and only via archive-then-replace
  (the superseded catalog is archived to `cleophis-models` before the new
  one is written) — publish.py (A5) owns this immutability guard.
- **Artifact base URL:** a single config value, currently
  `https://f005.backblazeb2.com/file/cleophis-dist` (native B2 URL today,
  Cloudflare in front of the same bucket later — see
  `src-tauri/src/catalog_dist.rs::ARTIFACT_BASE_URL`, the app-side pin).
  Every catalog `path` is relative to this base; the pipeline's `B2_ENDPOINT`
  (used for the S3-compatible upload API) is a *different* value from this
  public read URL — don't conflate them.
- **No B2/S3 credentials ever ship in the app.** They live only in this
  directory's gitignored `.env`, read by the upload/publish/verify steps.
  Public `GET` + sha256 + catalog signature is the app's entire trust
  model for these artifacts (`mint_download_url`/`DownloadAuth` is retired
  for catalog artifacts specifically — base + adapter — per the spec).

## Directory layout

```
tools/pipeline/
  requirements.txt     # this pipeline's own deps (committed)
  setup_tools.sh        # idempotent tooling setup (committed)
  .env.example           # template — copy to .env, fill in, never commit .env
  README.md               # this file (committed)
  .venv/                    # Python venv (gitignored, made by setup_tools.sh)
  llama.cpp/                 # pinned checkout + bin/llama-quantize (gitignored)
  work/                        # large scratch space for build steps (gitignored)
```
