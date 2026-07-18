# Verification — Security hardening (Phase 1)

Branch `feat/phase1-hardening` (PR #10). Second remediation wave from the
2026-07-18 audit — the Low items best done before external distribution.

## What shipped

| Fix | Commit | Audit item |
|-----|--------|-----------|
| Re-verify model sha256 at load (session-cached) | `59abb76` | sec-rust #5 (TOCTOU / arbitrary GGUF) |
| Kill orphan llama-server by tracked PID, not image name | `59abb76` | sec-rust #3 (kills unrelated servers) |
| Delete dormant one-time-payment webhook branch | `94b9046` | sec-edge #2 (latent lifetime over-grant) |
| Devices table caps (length + 50 rows/user) — migration 0008 | `4ae3d5e` | sec-db L-1 (storage abuse) |
| escapeHtml on all catalog innerHTML interpolations (+ cover src) | `f92be4b`, `d5236ab` | sec-client Low (defense-in-depth XSS) |

## Live verification

- **Re-verify at load:** hashes the resolved model path (streaming, 256 KiB
  buffer) against the catalog hero's pinned sha256, case-insensitive, BEFORE
  spawning llama-server; session-cached so the ~2 GB file is hashed once per
  process (watchdog respawns skip it). Mismatch → refuse to spawn,
  `engine-failed` emitted (fail-closed). `sha256: None` fails open (dev
  catalogs only; the download path requires and enforces the hash). Full Rust
  suite **115 pass**.
- **Kill-by-PID:** pid written to `temp/cleophis-llama.pid` after spawn, removed
  on kill; startup sweep kills only that pid double-filtered
  (`PID eq N` AND `IMAGENAME eq llama-server.exe` — PID-reuse-safe), absolute
  taskkill path preserved; absent/garbage pid → no-op (no more kill-all).
  Strictly better than the old image-name sweep.
- **Webhook branch deletion (redeployed 201), tested live:** a legacy
  `checkout.session.completed` event now returns `200 {received:true}` and
  writes **zero** entitlements (probe confirmed `n=0`) — clean ack, no Stripe
  retry storm, no one-time grant; a subscription `invoice.paid` still grants
  (`source=purchase`, `~+30d`). `EXPECTED` registry removed; signature verify /
  env fail-fast / `SUB_MODELS` / `apply_subscription_period` RPC intact.
  Grandfathered lifetime rows untouched (only the minting code was removed).
- **Devices caps (0008), applied live:** pre-checked the populated table (7
  rows, 0 violations, max 1/user) before the validating ALTER; applied; verified
  3 length constraints + `devices_cap_check` trigger present and EXECUTE revoked
  from authenticated. Cap keys on the real `(user_id, fingerprint)` unique
  constraint so device re-registration (UPSERT) is exempt; only genuinely-new
  device floods are bounded at 50.
- **escapeHtml:** helper covers all five entities; every catalog-origin
  `innerHTML` interpolation wrapped (card/drawer text, `data-id`, cover `src`,
  category class); `textContent` sinks (nickname, errors, chat) left alone; a
  no-op for current ASCII catalog values, so no layout change.

## Review

Opus whole-branch review: **Approved**, all five correct and regression-free.
Only actionable caveat was the validating length ALTER in 0008 — resolved by
the live pre-check above (no violating rows). PID-file-shared-across-instances
and `sha256:None` fail-open noted as acceptable by-design tradeoffs.

## Deferred

- **MSI code-signing** — needs a purchased code-signing certificate (Azure
  Trusted Signing ~$10/mo, or an OV/EV cert). Config wiring is a one-step
  follow-up once a cert/thumbprint exists; unsigned is fine for internal
  installs, a friction point only for external distribution.
- Phase-2 Info items (llama-server CORS live check, `CLEOPHIS_SUPABASE_*`
  debug-gating, column-scoped profiles UPDATE, truncate B2 error logs).
