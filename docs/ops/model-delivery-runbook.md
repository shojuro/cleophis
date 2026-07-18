# Model delivery runbook — Cleophis CDN

How model bytes get from a build machine to a paying user's disk, and how to
add a new model to the pipeline. Source of truth for the pieces this
document describes: `supabase/functions/download-url/index.ts`,
`tools/deploy-download-url.sh`, `src-tauri/resources/catalog.json`.

## Architecture in 5 lines

1. Model files live in a **private** Backblaze B2 bucket, `cleophis-models`,
   under `models/<model_id>/<version>/<file>`.
2. The Supabase edge function `download-url` (Deno) is the only thing that
   can mint access: it verifies the caller's Supabase JWT, checks an
   `entitlements` row exists for `(user_id, model_id)` (unexpired), then
   asks B2 for a **prefix-scoped** download authorization limited to that
   model's folder.
3. The minted authorization is a B2 download token valid for **6 hours**
   (`DOWNLOAD_AUTH_VALID_SECONDS = 21600`), returned alongside the file URL
   and expected `fileBytes` — never a bucket-wide credential.
4. The app's download worker (`src-tauri/src/cloud/download.rs`) streams the
   file using that token, with byte-range **resume** (`.part` files) and a
   **sha256** integrity check against the value pinned in `catalog.json`
   before the file is renamed into place.
5. Nothing else touches B2 directly — the app never sees the B2 account
   key, only the short-lived, model-scoped token the edge function hands
   back.

## Adding a new model

1. **Compute the sha256** of the finished `.gguf` file locally before
   uploading anything (`sha256sum <file>` or equivalent) — this is the
   value that goes into `catalog.json` and is what the client verifies
   against after download.

2. **Upload to B2** at `models/<model_id>/<version>/<file>` in the
   `cleophis-models` bucket, e.g.
   `models/socratic-tutor/1/Llama-3.2-3B-Instruct-Q4_K_M.gguf` (the hero
   model's actual path, per the `MODELS` registry below). Two supported
   approaches — either is fine, pick whichever tooling is on hand:

   - **curl single-shot** (the pattern used for the hero model upload —
     B2 native API, three calls):
     ```bash
     # 1. Authorize the account (returns apiUrl + authorizationToken)
     curl -s -u "$B2_KEY_ID:$B2_APP_KEY" \
       https://api.backblazeb2.com/b2api/v2/b2_authorize_account

     # 2. Get a one-time upload URL + token for the bucket
     curl -s -H "Authorization: $AUTH_TOKEN" \
       -d "{\"bucketId\":\"$B2_BUCKET_ID\"}" \
       "$API_URL/b2api/v2/b2_get_upload_url"

     # 3. Upload the file in one shot, headers carry the destination name
     #    and a sha1 B2 verifies server-side against the uploaded bytes
     curl -s -T "<local-file>" \
       -H "Authorization: $UPLOAD_AUTH_TOKEN" \
       -H "X-Bz-File-Name: models/<model_id>/<version>/<file>" \
       -H "Content-Type: b2/x-auto" \
       -H "X-Bz-Content-Sha1: <sha1-of-file>" \
       "$UPLOAD_URL"
     ```
     B2 rejects the upload if the `X-Bz-Content-Sha1` header doesn't match
     what it received — that's the server-side integrity check on the way
     up (separate from the sha256 the client checks on the way down).
   - **rclone** with a configured B2 remote — simpler for repeated uploads,
     same destination-prefix convention.

3. **Add the model to the edge function's `MODELS` registry**
   (`supabase/functions/download-url/index.ts`) — this in-source object is
   the function's source of truth for what actually exists in the bucket:
   ```ts
   const MODELS: Record<string, { prefix: string; file: string; bytes: number }> = {
     "socratic-tutor": {
       prefix: "models/socratic-tutor/1/",
       file: "Llama-3.2-3B-Instruct-Q4_K_M.gguf",
       bytes: 2019377696,
     },
     // "<new-model-id>": { prefix: "models/<id>/<version>/", file: "<file>", bytes: <exact-size> },
   };
   ```
   `bytes` must be the exact file size — the client cross-checks it against
   the mint response before starting the download.

4. **Redeploy the edge function**:
   ```bash
   tools/deploy-download-url.sh deploy
   ```
   This multipart-POSTs `supabase/functions/download-url/index.ts` to the
   Supabase Management API (`/v1/projects/<ref>/functions/deploy`). No
   secrets are touched by this step.

5. **Add the catalog entry** in `src-tauri/resources/catalog.json` (the
   client-facing metadata, distinct from the edge function's `MODELS`
   registry above):
   - `modelFile` — the local resource-relative filename the app writes to
     once downloaded (mirrors the uploaded file's basename).
   - `fileBytes` — same exact byte count as `MODELS[...].bytes` in the edge
     function. Keep these two numbers in sync manually; nothing enforces it
     automatically.
   - `sha256` — the value computed in step 1.
   - `version` — matches the `<version>` path segment used on B2 and in the
     edge function's `prefix`.

6. **Rebuild the app** so the updated `catalog.json` ships in the bundle
   (the model bytes themselves are never bundled — only the metadata that
   describes how to fetch and verify them; see the thin-installer note
   below).

## Cloudflare swap — LIVE since the domain milestone (dl.cleophis.com)

The token scheme is **hostname-agnostic by design**: the edge function
builds the download URL from `B2_DOWNLOAD_BASE_URL` if set, falling back to
B2's native `downloadUrl` from `b2_authorize_account` otherwise —
```ts
const base = (readEnv("B2_DOWNLOAD_BASE_URL") || auth.downloadUrl).replace(/\/+$/, "");
const url = `${base}/file/${b2BucketName}/${model.prefix}${model.file}`;
```
The swap was exercised for real in the Cloudflare milestone:
`B2_DOWNLOAD_BASE_URL=https://dl.cleophis.com` is set as a function
secret, and downloads flow app → `dl.cleophis.com` (Cloudflare, proxied
CNAME → `f005.backblazeb2.com`) → B2. The per-request B2 authorization
token rides the `Authorization` header unchanged — Cloudflare passes it
through. Rollback is deleting the secret (URLs fall back to B2's native
hostname; tokens keep working).

Cloudflare-side configuration (zone `cleophis.com`, Free plan, managed via
the scoped API token `CLOUDFLARE_API_TOKEN` in `~/.env`):
- `dl` CNAME → `f005.backblazeb2.com`, **proxied**; SSL mode Full
  (strict); Always Use HTTPS on.
- A WAF custom rule **skips** bot/challenge products (`bic`,
  `securityLevel`, `uaBlock`, `zoneLockdown`, `waf`, `rateLimit`) for
  `http.host eq "dl.cleophis.com"` — the Rust ureq client is not a
  browser and must never be challenged.
- The 2 GB GGUF exceeds the Free plan's 512 MB cache ceiling, so requests
  pass through uncached. That's fine: Backblaze waives egress to
  Cloudflare (Bandwidth Alliance), so the $-cap pressure drops to Class-B
  transaction fees. The **$3/day cap stays** as a backstop (see below).

## Ops notes

- **B2 daily download caps — the $-cap lesson.** B2's free tier caps daily
  download bandwidth; once exceeded, every request gets `HTTP 403
  {"code":"download_cap_exceeded", ...}` — B2 was working correctly, the
  account was being throttled, not down. The download worker's original
  retry path treated this identically to a flaky connection ("Download
  failed — check your connection and retry."), which is actively
  misleading for a definitive server refusal. Fixed in
  `src-tauri/src/cloud/download.rs` (commit `03088ef`): 403/404 are now
  classified as a "definitive refusal" distinct from a transport error —
  the give-up message says "the server refused the request (HTTP {status}).
  This can be a temporary account limit; try again later," and two
  consecutive definitive refusals fast-fail instead of burning a third
  retry. The account's daily cap was raised to **$3/day** to give headroom
  above the default free-tier ceiling (which 403'd after roughly 1 GB of
  downloads in a day) — check the B2 bucket's cap setting before a demo or
  load test that will pull the model repeatedly.
- **Key hygiene.** The edge function's B2 application key
  (`B2_KEY_ID`/`B2_APP_KEY`) only ever calls `b2_authorize_account` and
  `b2_get_download_authorization` — it needs `readFiles`/`shareFiles`
  capability, never `writeFiles`. Keep it scoped that way. The separate key
  used to upload model files (step 2 above) needs `writeFiles` but is only
  used interactively at upload time from a workstation — it is not
  referenced anywhere in deployed code or secrets, so it is safe (and
  preferable) to delete or rotate it after each upload session rather than
  leaving a standing write-capable credential around.
- **Secrets live in function env only.** `B2_KEY_ID`, `B2_APP_KEY`,
  `B2_BUCKET_ID`, `B2_BUCKET_NAME`, and the optional
  `B2_DOWNLOAD_BASE_URL` are set via `tools/deploy-download-url.sh
  secrets_set`, which reads them from the calling shell's environment and
  POSTs them to the Supabase Management API — they are never echoed,
  written to a file the script controls, or committed anywhere.
  `SUPABASE_URL`/`SUPABASE_SERVICE_ROLE_KEY` are Supabase-provided function
  env, not set by this script. `SUPABASE_ACCESS_TOKEN` (the Management API
  credential used to run the script itself) is sourced from `~/.env`,
  outside the repo.
