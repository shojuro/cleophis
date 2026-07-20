# Offline Auth — Design Spec

**Date:** 2026-07-20
**Status:** Approved (design), pending implementation plan
**Scope:** self-contained in the `src-tauri/src/cloud` auth module + FE auth surface

## 1. Goal

Cleophis is offline-first, but `sign_in()` always calls the network (Supabase GoTrue), so a **signed-out** user cannot sign back in while offline (airplane mode / no internet), locking them out of their own local models, packs, and chats. This milestone lets a signed-out user sign back in *while offline*, for any account that has authenticated online at least once on this device — and enforces strong passwords, since the offline verifier is brute-forceable with device access.

Already handled today (not in scope): a user who checked "Keep me signed in" survives offline via `restore()` → `offline_cached_info()` (mode `offlineCached`). The gap is offline **re-sign-in** after sign-out / no-remember / a different account on a shared device.

**Non-goals (YAGNI):** cross-device password sync; protecting against an attacker who already owns the device's OS login (they have the plaintext packs already); offline sign-in for an account that has never been online on this device; a server-side password policy rewrite (the server already returns `WeakPassword`).

## 2. Threat model & invariants

**The core safety invariant (most important thing in this spec):** *The server is authoritative whenever it is reachable.* Offline verification fires **only** on a transport/network error (`CloudError::Offline`) — **never** on any server *response*, including a `401/400` credential rejection. This is the line between "offline convenience" and "offline auth bypass." If the server says no, we say no.

**Second invariant:** *Per-account isolation.* Account B's verifier must never open account A's session. Verifiers and cached profiles are keyed by the authoritative `user_id`, mirroring the A6 per-account pack/chat isolation.

**Honest caveats (documented, not "fixed"):**
- The verifier is a salted **Argon2id** hash — non-reversible. Cracking it requires device + OS access and is bounded by the Argon2id cost + password entropy (hence zxcvbn enforcement).
- An attacker with the victim's OS login already has the plaintext packs (`packs/<uid>/` is not encrypted) and can attack the keyring hash directly — no app-level throttle stops them. This is the same tradeoff every password manager accepts; we document it rather than pretend to fix it.
- Offline entitlements = the cached snapshot within the existing grace window (`grace_expired` / `OFFLINE_GRACE_DAYS`). Offline sign-in never extends entitlements beyond grace.
- A password changed on **another** device is unknown here until this device reconnects (inherent to offline auth).

## 3. Components

### `cloud/verifier.rs` (new)
- `derive_verifier(password: &str) -> StoredVerifier` — Argon2id over the **normalized** password (§4) with a fresh random salt. Params: **m = 19456 KiB (19 MiB), t = 2, p = 1** (≈ OWASP; ~0.3–0.5 s/guess on the low-end hardware floor, where sign-in shares RAM with an imminent model load).
- `verify(password: &str, stored: &StoredVerifier) -> bool` — constant-time comparison of the re-derived hash.
- `StoredVerifier { version: u8, salt: Vec<u8>, hash: Vec<u8> }`, serde-serializable. `version` lets params rise later without a migration (an old verifier still verifies with its own recorded params; new enrollments use the new params).

### Keyring — per-account verifier
The keyring today holds a single refresh-token entry (`Entry::new(KEYRING_SERVICE, KEYRING_USER)`). Add a **per-account** verifier entry keyed by `user_id`: `Entry::new(KEYRING_SERVICE, &format!("verifier:{user_id}"))` storing the serialized `StoredVerifier`. The refresh-token entry is untouched. `save_verifier(user_id, &v)`, `load_verifier(user_id) -> Option<StoredVerifier>`, `delete_verifier(user_id)`.

### Per-account auth cache — `auth-cache/<user_id>.json` (app-data)
Mirrors `conversations/<user_id>.db` and `packs/<user_id>/`. One small JSON per account:
```
{ email, nickname, entitlements, last_online_auth, failed_attempts, last_failed_at }
```
Read to open an offline session for the target account; `failed_attempts`/`last_failed_at` back the persisted throttle (§7). Written on every successful online auth. (This supersedes the single-slot `CloudCache` for the offline-sign-in path; the existing single cache used by `restore()` can remain for the remember-me path, or be unified — an implementation detail for the plan.)

### `zxcvbn` crate
`check_strength(password, &[email, nickname]) -> StrengthResult { score: u8 /*0..4*/, ok: bool, feedback }`. Policy: **length ≥ 12 AND score ≥ 3**. Used both as a Rust guard (authoritative, at enrollment/change) and to feed the FE live meter.

## 4. Normalization (slice 1, correctness-critical)

- **Email (lookup key):** trim + lowercase before storing in / comparing against `auth-cache/*.json`, matching how GoTrue treats emails. `Alice@x.com` typed offline must match `alice@x.com` enrolled online.
- **Password:** apply the **RFC 8265 `OpaqueString` profile** (NFC normalization, preserve case, width-map) before **both** `derive_verifier` and `verify`. The same password typed on Windows vs. macOS vs. the future phone can arrive as different Unicode compositions and would otherwise fail against a verifier enrolled on the other platform — a silent, field-only bug.
- **Guardrail:** normalization applies **only** to our local verifier. The password bytes sent to the server (`auth::sign_in_password` / `sign_up`) stay byte-for-byte identical to today — we do not re-normalize what Supabase sees, so online auth behavior is unchanged.

## 5. Enrollment (silent, behind one-time consent)

On any successful **online** `sign_in` / `sign_up` (the plaintext password is in memory at that moment):
1. `derive_verifier(password)` → `save_verifier(user_id, v)` in the keyring.
2. Write `auth-cache/<user_id>.json` with `email, nickname, entitlements, last_online_auth = now`, and reset `failed_attempts = 0`.

On an online **password change**: re-derive + overwrite the verifier (same-device change covered; other-device change is the §2 caveat).

Enrollment is invisible after the one-time consent (§9).

## 6. Offline sign-in flow — `sign_in(email, password, remember)`

1. Try the server (`auth::sign_in_password`) exactly as today.
2. **Success** → the existing online path, **plus** refresh the verifier + auth-cache (§5).
3. **`CloudError::Offline`** (transport error only):
   a. Resolve `email → user_id` by scanning the small `auth-cache/*.json` set with the normalized email (§4). No match → *"Sign in online once on this device first."*
   b. `load_verifier(user_id)`. None → same *"…online once on this device first."*
   c. Apply the persisted throttle delay (§7).
   d. `verify(password, stored)` (constant-time):
      - **Match** → load `auth-cache/<user_id>.json` → open an `offlineCached` session (entitlements + `grace_expired` computed from the snapshot's `last_online_auth`). Reset `failed_attempts = 0`.
      - **No match** → increment `failed_attempts`, set `last_failed_at = now`, return *"Incorrect password."*
4. **Any server response** (`InvalidCredentials`, `RateLimited`, `EmailNotConfirmed`, `Api{…}`, …) → the normal online error path. **Never** fall back to the verifier. (Invariant §2.)

## 7. Progressive throttle (persisted)

Per-account, stored in `auth-cache/<user_id>.json` (`failed_attempts`, `last_failed_at`). After **5** consecutive offline failures, impose a growing pre-`verify` delay (2 → 4 → 8 s, capped ~30 s). Reset on any success (offline match or online auth). **Never hard-locks** — hard lockout on a local device is a denial-of-service gift to a user who fat-fingers their own password with no internet.

Honest scope: persisting closes the **process-restart** bypass (the realistic casual-attacker move — close and reopen the app). A determined attacker with file access can delete the counter or attack the keyring hash directly; the throttle is a nearly-free casual deterrent, not a hard boundary. Argon2id cost + password entropy remain the real defense.

## 8. Account removal (slice 2b)

"Remove account from this device" (profile menu; works offline; confirmation dialog). Deletes the keyring verifier (`delete_verifier(user_id)`) + the `auth-cache/<user_id>.json` entry. A checkbox additionally wipes that account's local data (`packs/<user_id>/`, `conversations/<user_id>.db`). If the account being removed is the currently-active session, sign out first. This is the reversal the consent copy promises, and is symmetric with the future §6 unpair story.

## 9. Front-end

- **Sign-up / change-password:** live `zxcvbn` strength meter; submit blocked until length ≥ 12 && score ≥ 3, showing zxcvbn feedback. The Rust guard is authoritative; the server `WeakPassword` path still handled.
- **Sign-in:** UI unchanged; the offline fallback is transparent → opens in `offlineCached` mode (the header already shows "offline, using saved account data"). Offline errors surface the two messages from §6.
- **One-time consent** at first online auth: *"To let you sign in when offline, Cleophis stores a secure, non-reversible credential on this device. You can remove this at any time by removing the account from this device. [Agree]"* — recorded once (a flag), invisible after.
- **Account removal** entry in the profile menu (§8).

## 10. Testing

- `verifier.rs`: derive→verify round-trip; wrong password fails; constant-time compare; versioned serialization round-trip; a normalized-password (NFC / case) pair that differs by composition still verifies; the server-facing password is *not* normalized (guardrail).
- Enrollment: online `sign_in`/`sign_up` writes verifier + auth-cache; password change overwrites the verifier.
- Offline flow: `Offline` + valid verifier + matching pw → `offlineCached` with correct entitlements/grace; wrong pw → error + `failed_attempts` bump; no verifier → "online once first"; **`InvalidCredentials` (and every other server response) never falls back**; email→user_id resolution across multiple accounts (normalized); **per-account isolation — account B's verifier cannot open account A's session**.
- Throttle: 5 consecutive failures → growing delay; reset on success; persists across a simulated restart.
- Strength: length/score enforcement + feedback; server `WeakPassword` still handled.
- Account removal: deletes verifier + auth-cache; optional local-data wipe; removing the active account signs out first.
- **Opus adversarial security review** (final gate, as delete_pack / A6 got), pointed at the two seams: (1) the Offline-vs-any-server-*response* branch — the fallback must be unreachable on any server reply; (2) per-account isolation — B's verifier opening A's session is the catastrophic bug class.

## 11. Implementation slices (ordered)

1. **`verifier.rs`** — Argon2id derive/verify + `StoredVerifier` (versioned) + RFC 8265 `OpaqueString` password normalization + tests.
2. **Storage** — keyring per-account verifier (`save/load/delete_verifier`) + per-account `auth-cache/<user_id>.json` read/write (incl. persisted throttle fields) in `store.rs`.
   2b. **Account removal** — delete verifier + auth-cache (+ optional local-data wipe); backend + wiring.
3. **Enrollment** — hook `sign_in`/`sign_up`/password-change to write the verifier + auth-cache.
4. **Offline fallback** — `session.rs`: email→user_id resolve, throttle, `verify`, open `offlineCached`. *Security-critical.*
5. **Strength** — `zxcvbn` Rust guard + enforcement (ordered ahead of enrollment reaching a user; single end-of-milestone MSI guarantees no build ships with enrollment-but-no-gate).
6. **Front-end** — consent (with the removal-exit copy), strength meter, offline error messages, account-removal menu entry.
7. **Opus security review** (two seams above) → MSI → user validation.

## 12. Out of scope / future

Cross-device password/verifier sync (ties into §6 LAN pairing); encrypting local packs at rest (separate, deeper boundary); per-message audit; biometric/OS-credential unlock as an alternative to the password verifier.
