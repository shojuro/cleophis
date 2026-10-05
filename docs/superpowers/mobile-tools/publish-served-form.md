# Publishing the served form: Qwen3-1.7B Q6_K + triage v3 (catalog v11)

This runbook takes the two pod-built triage artefacts from the private
`cleophis-models` bucket to the public `cleophis-dist` catalog, as signed
catalog **v11**. It carries the eleven artefacts of the live v10 catalog
forward unchanged and adds two:

| Artefact | Bucket path in `cleophis-dist` | sha256 | Bytes |
|---|---|---|---|
| Served base | `models/Qwen3-1.7B/v1/Qwen3-1.7B-Instruct-Q6_K.gguf` | `2588912fe87f55b8381b9fc8faacd0e905c9eb2db641a468be693f45ad6fc87a` | 1,673,006,944 |
| Triage adapter v3 | `adapters/triage/v3/Qwen3-1.7B/triage-v3-Qwen3-1.7B.gguf` | `5304e464cd485e8a7d8eb75083363e3cc4de0f665e2c785dbd1a1f7e93d13a20` | measured in step 3 |

**Who runs what.** Agents prepare. The founder runs every step that touches
a credential: the private-bucket fetch (step 1), `sign_catalog.py` (step 5)
and the live `publish.py` (step 6). No agent reads, prints or copies the
`.env`, the curator key file or any B2 key. No secret value appears in this
document; commands name variables, never values.

**Distribution-origin rule.** Every shipped byte comes from our buckets. The
served base is the pod-built file, never rebuilt here. Its hash is checked at
every hop: the pod's sidecar, the pinned literal below, `build_catalog.py`'s
manifest, `publish.py`'s pre-upload re-hash, and `verify_published.py`'s
public re-download.

## 0. Preconditions

- The M10 gate pod has uploaded, to `cleophis-models` under
  `lineage/cleophas-triage/served/`, the four objects
  `Qwen3-1.7B-Instruct-Q6_K.gguf`, `Qwen3-1.7B-Instruct-Q6_K.gguf.sha256`,
  `triage-v3-Qwen3-1.7B.gguf` and `triage-v3-Qwen3-1.7B.gguf.sha256`.
- The pipeline code from Phase 1g Task M3 is on the checkout you run from
  (`build_catalog.py --carry-forward`, `publish.py --carried-forward`, the
  quant-derived base subdir).
- A pipeline venv exists. No llama.cpp is needed for this runbook, so the
  light form is enough:

  ```bash
  cd "/mnt/c/Users/JM505 Computers/dev/cleophis-mobile/tools/pipeline"
  python3 -m venv .venv && ./.venv/bin/pip install -r requirements.txt
  ```

- `sign_catalog.py` and `publish.py` read `tools/pipeline/.env` next to
  themselves (`CURATOR_KEY_FILE`, `B2_ENDPOINT`, `B2_KEY_ID`, `B2_APP_KEY`,
  `B2_MODELS_KEY_ID`, `B2_MODELS_APP_KEY`). The triage repo's
  `~/cleophas-triage/pipeline/env.py` reads the desktop checkout's copy,
  `/mnt/c/Users/JM505 Computers/dev/cleophis/tools/pipeline/.env`. If the
  mobile checkout has no `.env` of its own, the founder provides one there
  (or runs steps 5 and 6 from a checkout that has it). Agents never do this.

All commands below run from the mobile checkout's pipeline directory:

```bash
cd "/mnt/c/Users/JM505 Computers/dev/cleophis-mobile/tools/pipeline"
```

## 1. Fetch from `cleophis-models` (founder)

This uses the triage repo's key loader, `pipeline.env.require`, with the
models-bucket pair. It prints only object names.

```bash
mkdir -p work/out/Qwen3-1.7B/Q6_K work/out/Qwen3-1.7B/adapter
./.venv/bin/python3 - <<'PY'
import sys
sys.path.insert(0, "/home/penguinzyue/cleophas-triage")
from pipeline.env import require
import boto3

k = require("B2_ENDPOINT", "B2_MODELS_KEY_ID", "B2_MODELS_APP_KEY")
s3 = boto3.client(
    "s3",
    endpoint_url=k["B2_ENDPOINT"],
    aws_access_key_id=k["B2_MODELS_KEY_ID"],
    aws_secret_access_key=k["B2_MODELS_APP_KEY"],
)
prefix = "lineage/cleophas-triage/served/"
for name, dest in [
    ("Qwen3-1.7B-Instruct-Q6_K.gguf", "work/out/Qwen3-1.7B/Q6_K/"),
    ("triage-v3-Qwen3-1.7B.gguf", "work/out/Qwen3-1.7B/adapter/"),
]:
    for obj in (name, name + ".sha256"):
        s3.download_file("cleophis-models", prefix + obj, dest + obj)
        print("fetched", prefix + obj)
PY
```

## 2. Verify the hashes (anyone)

Each file must match its pod sidecar AND the registered literal. Stop on any
mismatch. Do not publish a file that fails either check.

```bash
B=work/out/Qwen3-1.7B/Q6_K/Qwen3-1.7B-Instruct-Q6_K.gguf
A=work/out/Qwen3-1.7B/adapter/triage-v3-Qwen3-1.7B.gguf
B_SHA=$(sha256sum "$B" | cut -c1-64); A_SHA=$(sha256sum "$A" | cut -c1-64)
test "$B_SHA" = 2588912fe87f55b8381b9fc8faacd0e905c9eb2db641a468be693f45ad6fc87a && echo "base: registered sha OK"
test "$B_SHA" = "$(cut -c1-64 "$B.sha256")" && echo "base: pod sidecar OK"
test "$(stat -c %s "$B")" = 1673006944 && echo "base: size OK"
test "$A_SHA" = 5304e464cd485e8a7d8eb75083363e3cc4de0f665e2c785dbd1a1f7e93d13a20 && echo "adapter: registered sha OK"
test "$A_SHA" = "$(cut -c1-64 "$A.sha256")" && echo "adapter: pod sidecar OK"
```

Five `OK` lines are required. A missing line means a failed check.

## 3. Write the two manifests (anyone)

The base manifest is fully pinned:

```bash
cat > work/out/Qwen3-1.7B/Q6_K/manifest.json <<'EOF'
{
  "name": "Qwen3-1.7B-Instruct-Q6_K.gguf",
  "kind": "base",
  "base_model": "Qwen3-1.7B",
  "quant": "Q6_K",
  "sha256": "2588912fe87f55b8381b9fc8faacd0e905c9eb2db641a468be693f45ad6fc87a",
  "size": 1673006944,
  "license": "Apache-2.0",
  "source": "cleophis-models/lineage/cleophas-triage/served/Qwen3-1.7B-Instruct-Q6_K.gguf (built on the M8/M9/M10 pods, llama.cpp b10042)"
}
EOF
```

The adapter's size is not registered, so it is measured from the verified
file. The command below writes exactly this JSON, with `size` filled in:

```bash
A_BYTES=$(stat -c %s work/out/Qwen3-1.7B/adapter/triage-v3-Qwen3-1.7B.gguf)
cat > work/out/Qwen3-1.7B/adapter/manifest.json <<EOF
{
  "name": "triage-v3-Qwen3-1.7B.gguf",
  "kind": "adapter",
  "base_model": "Qwen3-1.7B",
  "adapter_name": "triage",
  "version": "v3",
  "sha256": "5304e464cd485e8a7d8eb75083363e3cc4de0f665e2c785dbd1a1f7e93d13a20",
  "size": ${A_BYTES},
  "license": "Apache-2.0",
  "source": "cleophis-models/lineage/cleophas-triage/served/triage-v3-Qwen3-1.7B.gguf"
}
EOF
cat work/out/Qwen3-1.7B/adapter/manifest.json
```

`build_catalog.py` ignores `source`. It keeps it only as provenance for the
reader.

## 4. Build catalog v11, carrying v10 forward (anyone)

Fetch the live catalog and its signature with public GETs (no
credentials). `--carry-forward` refuses unless the signature verifies under
the production curator public key, the one pinned in the app
(`crates/kpack-core/src/sign.rs`, `CURATOR_PUBLIC_KEY`).

```bash
DIST=https://cleophis-dist.s3.us-east-005.backblazeb2.com
PUBKEY=158cb99e9756e2e4d01d88b7ecfeb99a76547821ffe9f9f1985316c5451cd0c0
mkdir -p work/live
curl -fsS "$DIST/catalog.json"     -o work/live/catalog.json
curl -fsS "$DIST/catalog.json.sig" -o work/live/catalog.json.sig

./.venv/bin/python3 build_catalog.py \
  --manifests work/out/Qwen3-1.7B/Q6_K/manifest.json \
              work/out/Qwen3-1.7B/adapter/manifest.json \
  --base-url "$DIST" \
  --carry-forward work/live/catalog.json \
  --carry-forward-pubkey "$PUBKEY"
```

Expected output:

```
[carry-forward] work/live/catalog.json verifies; carrying 11 artifact(s) from catalog_version=10
[version] currently published catalog_version=10 at https://cleophis-dist.s3.us-east-005.backblazeb2.com -> using 11
[catalog] wrote .../work/catalog/catalog.json (catalog_version=11, 13 artifact(s))
```

The builder refuses, and writes nothing, if any of these hold:

- the carried file's signature does not verify;
- the carried file is not the live version, which would drop later artefacts;
- a carried entry does not re-derive unchanged;
- a bucket path repeats;
- a manifest's `name` is not the derived basename.

Check that the first eleven entries are exactly v10's and the last two are
the new pair:

```bash
./.venv/bin/python3 - <<'PY'
import json
old = json.load(open("work/live/catalog.json"))["artifacts"]
new = json.load(open("work/catalog/catalog.json"))
assert new["catalog_version"] == 11 and new["artifacts"][:11] == old, "carry-forward mismatch"
for a in new["artifacts"][11:]:
    print(a["path"], a["sha256"], a["size"], a["kind"], a["version"])
PY
```

## 5. Sign (founder)

`sign_catalog.py` reads `CURATOR_KEY_FILE` from `tools/pipeline/.env` and
writes `work/catalog/catalog.json.sig`. Then verify it with the public key
only.

```bash
./.venv/bin/python3 sign_catalog.py --catalog work/catalog/catalog.json
./.venv/bin/python3 sign_catalog.py --catalog work/catalog/catalog.json --verify-with "$PUBKEY"
```

## 6. Publish (founder)

First the dry run. It needs no credentials and touches no network. The two
new files are re-hashed locally. The eleven carried entries are listed as
"would confirm", because they are already in the bucket and are never
uploaded or resolved locally.

```bash
./.venv/bin/python3 publish.py --carried-forward work/live/catalog.json --verify-pubkey "$PUBKEY" --dry-run
```

Then the live publish, which reads the B2 pairs from `tools/pipeline/.env`:

```bash
./.venv/bin/python3 publish.py --carried-forward work/live/catalog.json --verify-pubkey "$PUBKEY"
```

The live publish runs in this order:

1. HEAD each carried artefact. It refuses before any upload if one is
   missing or has different sha256 metadata.
2. Upload the two new artefacts to their immutable paths. A re-run after an
   interruption skips whatever already landed with a matching sha.
3. Archive the live v10 pair to
   `cleophis-models/archive/catalogs/v10/`.
4. Put the v11 `catalog.json`, then its `.sig`.

## 7. Verify from the public path (anyone)

This is the definition of done. It impersonates the app with only the base
URL and the public key. It re-downloads and re-hashes every artefact in v11,
about 12 GB, so run it on a good connection.

```bash
./.venv/bin/python3 verify_published.py --pubkey "$PUBKEY"
```

It must end with an `OK:` line and exit 0.

## Notes for the reviewer

- **Adapter manifest schema.** `adapter_name` matches
  `[a-z][a-z0-9]*(-[a-z0-9]+)*`. `version` matches `v<N>(.<N>)*`. The
  basename is `<adapter_name>-<version>-<base_model>.gguf` at
  `adapters/<adapter_name>/<version>/<base_model>/`. A kind `adapter` with
  neither field is `behavioral` + `v1`. A kind `contract-adapter` is always
  `contract` and still accepts the legacy `dist_version`.
- **Base manifest schema.** `quant` matches `[A-Za-z0-9_]+`. The basename is
  `<base_model>-Instruct-<quant>.gguf` at `models/<base_model>/v1/`.
  `publish.py` resolves it locally under `work/out/<base_model>/<quant>/`.
- **Catalog entry `version` field.** The new triage entry carries `"v1"`,
  like every live kind `adapter` entry, `tutor-v1.3` included. The app reads
  nothing from this field. The adapter's own version lives in its path.
- **What this does not do.** Publishing v11 does not by itself make the app
  install the triage pair. The Phase 1g survey found three app-side gaps:
  the empty hero `base_model`, selection by kind plus base, and the bundled
  basenames. Other Phase 1g tasks own those.
- **Order is load-bearing.** Today's app picks the FIRST entry matching
  kind plus `base_model` (`downloadHeroPair` in `src/app.js` uses
  `arts.find`). `build_catalog.py` puts carried entries first, so in v11 the
  Q4 base and the behavioral adapter still come before the Q6_K base and the
  triage adapter for `Qwen3-1.7B`. Shipped apps therefore keep downloading
  exactly what they download from v10. Do not reorder the catalog by hand.
