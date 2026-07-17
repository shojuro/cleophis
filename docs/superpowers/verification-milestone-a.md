# Milestone A (Supabase Auth + Entitlements) — Verification Record

Date: 2026-07-17 · Branch: `feat/supabase-auth` · Project: `isltexsxpysxqewjsryv` (Cleophis)

| # | Check | Result | Evidence |
|---|---|---|---|
| 1 | Schema applied (4 migrations) with RLS | **PASS** | Management API 201×4; pg_tables shows RLS on all 3 tables; 7 policies present |
| 2 | Security advisors | **PASS** | 0 lints after 0004 (trigger-fn RPC execute revoked) |
| 3 | Unit + mock-server suite | **PASS** | 55/55 (auth mapping, store, rest shapes, session lifecycle, cache merge) |
| 4 | Live integration roundtrip | **PASS** | `live_auth_and_entitlements_roundtrip` vs real project: signup → trigger profile → grant → idempotent duplicate → RLS purchase fence (403) → sign-out; test user removed server-side |
| 5 | In-app online flows (user-verified) | **PASS** | Real account creation → instant sign-in; hero pay/download/chat unchanged; sign-out locks; sign-in restores library from server |
| 6 | Server rows | **PASS** | profiles.nickname "shojuro" (trigger); entitlements trial+library; devices row GTX 1650/32 GB/mid (upsert) |
| 7 | **Airplane-mode regression (installed app, user-verified)** | **PASS** | Auto-signed-in offline from cached session, library intact, chat streams, offline stub download queued |
| 8 | Offline grant flush | **PASS** | `ap-bio/library` row appeared server-side after online relaunch (queue → flush verified) |
| 9 | MSI rebuild + reinstall | **PASS** | `Cleophis_0.1.0_x64_en-US.msi` (2,021,289,984 B), installed and running all above |
| 10 | CSP / offline boundary | **PASS** | `tauri.conf.json` untouched this milestone; all network code confined to `src-tauri/src/cloud/` |

Deferred (recorded in ledger for Milestone B): per-user row caps / model_id FK, devices DELETE policy, trial expiry server-side computation, pin 403 in RLS test assert, unify keyring test locks, temp-file orphan sweep.
