# Offline Auth Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a signed-out user sign back in while offline (for any account that has authenticated online at least once on this device), gated by a locally-stored Argon2id password verifier, with enforced strong passwords and a per-account removal path.

**Architecture:** A new `cloud/verifier.rs` derives/verifies an Argon2id hash of the (normalized) password. On successful online auth we enroll: store the verifier in the OS keyring (per-account) and a small `auth-cache/<user_id>.json` profile snapshot. `sign_in` falls back to the local verifier **only** on a transport error (never on a server rejection). A `zxcvbn` strength gate runs at sign-up/change. Everything is self-contained in `src-tauri/src/cloud` + the FE auth surface.

**Tech Stack:** Rust (Tauri 2 backend), `argon2` crate (RustCrypto, pure-Rust Argon2id), `zxcvbn` crate (pure-Rust strength estimation), `unicode-normalization` crate (NFC), `keyring` crate (already a dep), vanilla JS front-end.

## Global Constraints

- Repo path contains a space: `/mnt/c/Users/JM505 Computers/dev/cleophis` — always quote it.
- Branch: `feat/offline-auth` (already created off main; the spec commit is its first commit).
- **Core invariant:** the server is authoritative on any *response*. Offline verification fires **only** on `CloudError::Offline` (transport error), never on `InvalidCredentials`/`RateLimited`/`EmailNotConfirmed`/`Api{..}` or any other server reply.
- **Per-account isolation:** verifier + auth-cache keyed by the authoritative `user_id` (from the authenticated session, never FE input) — same A6 pattern as `packs/<user_id>/` and `conversations/<user_id>.db`.
- Argon2id params: **m = 19456 KiB, t = 2, p = 1**; `StoredVerifier` carries a `version: u8` so params can change later without migration.
- Password normalization (RFC 8265 OpaqueString, load-bearing part = **NFC**) applies **only** to the local verifier (derive + verify). The bytes sent to the server stay byte-for-byte as today.
- Email lookup key: **trim + lowercase**.
- Strength policy: **length ≥ 12 AND zxcvbn score ≥ 3** (of 4).
- Throttle: after 5 consecutive offline failures, growing pre-verify delay (2→4→8 s, capped 30 s); reset on any success; **never hard-lock**; state persisted in the auth-cache JSON.
- Consent copy (verbatim): *"To let you sign in when offline, Cleophis stores a secure, non-reversible credential on this device. You can remove this at any time by removing the account from this device."*
- Rust verify command (run from bash): `PWSH='/mnt/c/Program Files/PowerShell/7-preview/pwsh.exe'; "$PWSH" -Command "cd 'C:\Users\JM505 Computers\dev\cleophis'; $env:LIBCLANG_PATH='C:\Program Files\LLVM\bin'; & 'C:\Users\JM505 Computers\.cargo\bin\cargo.exe' test -p cleophis <filter>"`. FE: `node -c src/app.js`.
- Commit trailer: `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`. Each task `git add`s ONLY its own files, never `-A`.
- Do NOT touch payment/download/kpack/CSP or the §7 convstore beyond what a task names.

---

### Task 1: `verifier.rs` — Argon2id derive/verify + password normalization

**Files:**
- Create: `src-tauri/src/cloud/verifier.rs`
- Modify: `src-tauri/src/cloud/mod.rs` (add `pub mod verifier;`)
- Modify: `src-tauri/Cargo.toml` (add `argon2`, `unicode-normalization` deps)
- Test: inline `#[cfg(test)]` in `verifier.rs`

**Interfaces:**
- Produces:
  - `pub struct StoredVerifier { pub version: u8, pub salt: String, pub hash: String }` (serde `Serialize`/`Deserialize`; salt/hash base64 or the argon2 PHC string — see below)
  - `pub fn normalize_password(password: &str) -> String` (NFC)
  - `pub fn derive_verifier(password: &str) -> Result<StoredVerifier, String>`
  - `pub fn verify(password: &str, stored: &StoredVerifier) -> bool`
  - `pub const VERIFIER_VERSION: u8 = 1;`
  - `pub const ARGON2_M_KIB: u32 = 19456; pub const ARGON2_T: u32 = 2; pub const ARGON2_P: u32 = 1;`

**Implementation notes:** Use the `argon2` crate's PHC-string API (`argon2::PasswordHasher`/`PasswordVerifier` with `argon2::password_hash::{SaltString, PasswordHash}`). The PHC hash string already encodes salt + params, so `StoredVerifier` can store `{ version, phc: String }` — simpler and self-describing (params travel with the hash, satisfying the versioning goal). Configure the `Argon2` instance with `Params::new(19456, 2, 1, None)` and `Algorithm::Argon2id`, `Version::V0x13`. `verify` re-parses the PHC string and uses `verify_password` (constant-time inside the crate). Both `derive_verifier` and `verify` call `normalize_password` first.

- [ ] **Step 1: Add deps.** In `src-tauri/Cargo.toml` `[dependencies]` add: `argon2 = "0.5"` and `unicode-normalization = "0.1"`. (`keyring` and `serde` are already present.)

- [ ] **Step 2: Write failing tests** (append to `verifier.rs`):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derive_then_verify_roundtrip() {
        let v = derive_verifier("correct horse battery staple").unwrap();
        assert_eq!(v.version, VERIFIER_VERSION);
        assert!(verify("correct horse battery staple", &v));
    }

    #[test]
    fn wrong_password_fails() {
        let v = derive_verifier("s3cret-passphrase-xyz").unwrap();
        assert!(!verify("s3cret-passphrase-xyj", &v));
    }

    #[test]
    fn nfc_normalization_matches_across_compositions() {
        // "é" as U+00E9 (precomposed) vs "e" + U+0301 (combining acute).
        let precomposed = "caf\u{00E9}-password-1234";
        let decomposed = "cafe\u{0301}-password-1234";
        assert_ne!(precomposed, decomposed); // different bytes...
        let v = derive_verifier(precomposed).unwrap();
        assert!(verify(decomposed, &v)); // ...but verify after NFC.
    }

    #[test]
    fn serde_roundtrip() {
        let v = derive_verifier("another-passphrase-9876").unwrap();
        let json = serde_json::to_string(&v).unwrap();
        let back: StoredVerifier = serde_json::from_str(&json).unwrap();
        assert!(verify("another-passphrase-9876", &back));
    }
}
```

- [ ] **Step 3: Run tests, verify they fail** (compile error / not defined):

Run: `... cargo test -p cleophis verifier`
Expected: FAIL (module/functions not defined).

- [ ] **Step 4: Implement `verifier.rs`** (the module body above the tests): the `StoredVerifier` struct, `normalize_password` (`use unicode_normalization::UnicodeNormalization; password.nfc().collect()`), `derive_verifier` (build `Argon2` with the params, `SaltString::generate`, `hash_password`, store the PHC string + `VERIFIER_VERSION`), `verify` (normalize, `PasswordHash::new(&stored.phc)`, `Argon2::default().verify_password` → bool; return false on any parse error). Wire `pub mod verifier;` in `mod.rs`.

- [ ] **Step 5: Run tests, verify pass.**

Run: `... cargo test -p cleophis verifier`
Expected: PASS (4 tests). Also `... cargo build -p cleophis` exit 0.

- [ ] **Step 6: Commit.**

```bash
git add src-tauri/src/cloud/verifier.rs src-tauri/src/cloud/mod.rs src-tauri/Cargo.toml src-tauri/Cargo.lock
git commit -m "feat(offline-auth): Argon2id password verifier + NFC normalization

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 2: Per-account storage — keyring verifier + `auth-cache/<user_id>.json`

**Files:**
- Modify: `src-tauri/src/cloud/store.rs`
- Test: inline `#[cfg(test)]` in `store.rs`

**Interfaces:**
- Consumes: `verifier::StoredVerifier` (Task 1).
- Produces:
  - `pub fn save_verifier(user_id: &str, v: &StoredVerifier) -> Result<(), CloudError>`
  - `pub fn load_verifier(user_id: &str) -> Option<StoredVerifier>`
  - `pub fn delete_verifier(user_id: &str)`
  - `pub struct AuthCacheEntry { pub user_id: String, pub email: String, pub nickname: String, pub entitlements: Vec<Entitlement>, pub last_online_auth: i64, pub failed_attempts: u32, pub last_failed_at: i64 }` (serde)
  - `pub fn auth_cache_dir(app_data: &Path) -> PathBuf` (returns `app_data.join("auth-cache")`)
  - `pub fn read_auth_cache(dir: &Path, user_id: &str) -> Option<AuthCacheEntry>`
  - `pub fn write_auth_cache(dir: &Path, entry: &AuthCacheEntry) -> Result<(), CloudError>`
  - `pub fn find_user_by_email(dir: &Path, email: &str) -> Option<AuthCacheEntry>` (trim+lowercase compare; scans `*.json`)
  - `pub fn normalize_email(email: &str) -> String` (trim + lowercase)

**Implementation notes:** Keyring per-account entry: `Entry::new(KEYRING_SERVICE, &format!("verifier:{user_id}"))`, `set_password(&serde_json::to_string(v))` / `get_password()` → parse. `delete_verifier` mirrors `delete_refresh_token` (ignore errors). Auth-cache files: `<dir>/<user_id>.json` written via `serde_json` (create the dir if missing). `find_user_by_email` reads each `*.json`, compares `normalize_email(entry.email) == normalize_email(query)`, returns the first match.

- [ ] **Step 1: Write failing tests** (append to `store.rs` tests, following the existing keyring-test serialization pattern noted at store.rs:155):

```rust
#[test]
fn verifier_keyring_roundtrip() {
    let _g = KEYRING_TEST_LOCK.lock().unwrap(); // reuse the module's serialize lock
    let uid = "verifier-test-uid-t2";
    delete_verifier(uid);
    let v = crate::cloud::verifier::derive_verifier("plan-test-pw-abcdef").unwrap();
    save_verifier(uid, &v).unwrap();
    assert!(verify("plan-test-pw-abcdef", &load_verifier(uid).unwrap()));
    delete_verifier(uid);
    assert!(load_verifier(uid).is_none());
}

#[test]
fn auth_cache_roundtrip_and_email_lookup() {
    let dir = temp_path("auth-cache-t2"); // a fresh temp dir
    let entry = AuthCacheEntry { user_id: "uid-A".into(), email: "Alice@Example.com".into(),
        nickname: "Alice".into(), entitlements: vec![], last_online_auth: 1000,
        failed_attempts: 0, last_failed_at: 0 };
    write_auth_cache(&dir, &entry).unwrap();
    assert_eq!(read_auth_cache(&dir, "uid-A").unwrap().email, "Alice@Example.com");
    // case/space-insensitive email lookup:
    assert_eq!(find_user_by_email(&dir, "  alice@example.com ").unwrap().user_id, "uid-A");
    assert!(find_user_by_email(&dir, "bob@example.com").is_none());
}
```

- [ ] **Step 2: Run, verify fail.** Run: `... cargo test -p cleophis store`. Expected: FAIL (undefined).

- [ ] **Step 3: Implement** the functions + `AuthCacheEntry` struct + `normalize_email` in `store.rs`. Import `use crate::cloud::verifier::{StoredVerifier, verify};` in tests only as needed; production code needs `StoredVerifier`.

- [ ] **Step 4: Run, verify pass.** Run: `... cargo test -p cleophis store`. Expected: PASS.

- [ ] **Step 5: Commit.**

```bash
git add src-tauri/src/cloud/store.rs
git commit -m "feat(offline-auth): per-account keyring verifier + auth-cache store

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 3: Password strength gate (`zxcvbn`) — ordered ahead of enrollment

**Files:**
- Create: `src-tauri/src/cloud/strength.rs`
- Modify: `src-tauri/src/cloud/mod.rs` (`pub mod strength;`), `src-tauri/src/cloud/commands.rs` (a `check_password_strength` command), `src-tauri/src/main.rs` (register command), `src-tauri/Cargo.toml` (`zxcvbn` dep)
- Test: inline `#[cfg(test)]` in `strength.rs`

**Interfaces:**
- Produces:
  - `pub struct StrengthResult { pub score: u8, pub ok: bool, pub feedback: Vec<String> }`
  - `pub fn check_strength(password: &str, user_inputs: &[&str]) -> StrengthResult` (ok = `password.chars().count() >= 12 && score >= 3`)
  - `#[tauri::command] fn check_password_strength(password: String, email: String, nickname: String) -> StrengthResult`

**Implementation notes:** `zxcvbn = "3"`. `zxcvbn::zxcvbn(password, &[email, nickname])` returns a score 0..4; map to `u8`. Feedback from `estimate.feedback()` warnings/suggestions → `Vec<String>`. The command lets the FE render a live meter; the same `check_strength` is the authoritative guard called at enrollment (Task 4) before deriving a verifier.

- [ ] **Step 1: Add dep** `zxcvbn = "3"` to `src-tauri/Cargo.toml`.

- [ ] **Step 2: Write failing tests**:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn strong_passphrase_passes() {
        let r = check_strength("wintergreen-diesel-canyon-42", &[]);
        assert!(r.ok, "score {} len ok expected pass", r.score);
    }
    #[test]
    fn short_fails_even_if_complex() {
        let r = check_strength("aB3$xY", &[]); // < 12 chars
        assert!(!r.ok);
    }
    #[test]
    fn common_complex_password_fails_on_score() {
        let r = check_strength("Password123!", &[]); // 12 chars but weak
        assert!(!r.ok, "zxcvbn should score this < 3");
    }
    #[test]
    fn user_inputs_penalized() {
        let r = check_strength("alice-smith-1988", &["alice", "alice@x.com"]);
        assert!(!r.ok);
    }
}
```

- [ ] **Step 3: Run, verify fail.** `... cargo test -p cleophis strength`. Expected: FAIL.

- [ ] **Step 4: Implement** `strength.rs` (`check_strength` + `StrengthResult`), the `check_password_strength` command in `commands.rs`, register in `main.rs` `generate_handler!`, `pub mod strength;` in `mod.rs`.

- [ ] **Step 5: Run, verify pass** + `... cargo build -p cleophis`. Expected: PASS, build exit 0.

- [ ] **Step 6: Commit.**

```bash
git add src-tauri/src/cloud/strength.rs src-tauri/src/cloud/mod.rs src-tauri/src/cloud/commands.rs src-tauri/src/main.rs src-tauri/Cargo.toml src-tauri/Cargo.lock
git commit -m "feat(offline-auth): zxcvbn password-strength gate (len>=12 && score>=3)

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 4: Enrollment — write verifier + auth-cache on successful online auth

**Files:**
- Modify: `src-tauri/src/cloud/session.rs` (the `sign_in`/`sign_up` success paths; any password-change path if present)
- Test: `src-tauri/src/cloud/session.rs` `#[cfg(test)]` or `integration_tests.rs`

**Interfaces:**
- Consumes: `verifier::derive_verifier`, `store::{save_verifier, write_auth_cache, auth_cache_dir, AuthCacheEntry}`, `strength::check_strength`.
- Produces: a private helper `fn enroll_verifier(&self, user_id: &str, email: &str, nickname: &str, password: &str, entitlements: &[Entitlement])` called from the online-success paths.

**Implementation notes:** In `session.rs`, after a successful online `sign_in`/`sign_up` establishes `user_id` (and the code already builds the online `SessionInfo` at ~line 482), call `enroll_verifier(...)`: (1) `derive_verifier(password)` → `store::save_verifier(user_id, &v)`; (2) build an `AuthCacheEntry { user_id, email: normalize_email(email)? keep original for display but store as given; …, last_online_auth: now(), failed_attempts: 0, last_failed_at: 0 }` and `write_auth_cache(&auth_cache_dir(&self.app_data), &entry)`. Failures here must NOT break online sign-in (log + continue — same degrade-gracefully contract as the existing cache writes). `sign_up` should already have passed the strength gate at the command/FE layer (Task 3/6); enrollment does not re-run zxcvbn but MUST NOT enroll a password the gate would reject if it can cheaply check — for safety, enrollment calls `check_strength` and skips verifier storage (logging) if not `ok`, so a weak password never becomes an offline verifier.

- [ ] **Step 1: Write failing test** — an integration-style test that drives an online `sign_in` (using the existing `test_support`/mock server harness in `integration_tests.rs`) and asserts `store::load_verifier(user_id)` is `Some` and `read_auth_cache(dir, user_id)` has the right email + `last_online_auth`. (Follow the existing mock-auth test pattern in `integration_tests.rs`.)

- [ ] **Step 2: Run, verify fail.**

- [ ] **Step 3: Implement** `enroll_verifier` + call it from the `sign_in`/`sign_up` online-success paths.

- [ ] **Step 4: Run, verify pass** + build.

- [ ] **Step 5: Commit.**

```bash
git add src-tauri/src/cloud/session.rs src-tauri/src/cloud/integration_tests.rs
git commit -m "feat(offline-auth): enroll verifier + auth-cache on online sign-in/up

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 5: Offline sign-in fallback (security-critical)

**Files:**
- Modify: `src-tauri/src/cloud/session.rs` (`sign_in`)
- Test: `session.rs`/`integration_tests.rs`

**Interfaces:**
- Consumes: `store::{find_user_by_email, load_verifier, read_auth_cache, write_auth_cache}`, `verifier::verify`, `store::grace_expired`.
- Produces: the offline branch of `sign_in`; a private `fn offline_sign_in(&self, email: &str, password: &str) -> Result<SessionInfo, CloudError>`.

**Implementation notes:** In `sign_in`, wrap the existing `auth::sign_in_password` call. Match its result:
- `Ok(tokens)` → existing online path + `enroll_verifier` (Task 4).
- `Err(CloudError::Offline)` → `self.offline_sign_in(email, password)`.
- `Err(other)` → return `other` unchanged (NEVER call `offline_sign_in`). **This branch is the core invariant.**

`offline_sign_in`: `find_user_by_email(dir, email)` → None ⇒ `Err(CloudError::Offline)` with a message mapped in the FE to "sign in online once on this device first" (or a dedicated `CloudError` variant `OfflineNoVerifier`). Load its verifier → None ⇒ same. Apply throttle: if `failed_attempts >= 5`, compute `delay = min(2^(failed_attempts-4) , 30)` seconds since `last_failed_at`; if not elapsed, sleep the remainder (or return a "try again in Ns" error — pick sleep for simplicity, capped 30 s). `verify(password, &v)`:
- true ⇒ reset `failed_attempts = 0`, `write_auth_cache`, set the in-memory session (user_id/nickname), return `offline_cached_info`-style `SessionInfo { mode: "offlineCached", entitlements: entry.entitlements, grace_expired: grace_expired(entry.last_online_auth, now()), .. }`.
- false ⇒ `failed_attempts += 1`, `last_failed_at = now()`, `write_auth_cache`, `Err(CloudError::InvalidCredentials)` (FE shows "Incorrect password").

- [ ] **Step 1: Write failing tests** (the security matrix):
  - `offline_error_plus_valid_verifier_opens_offline_session` (Offline + matching pw → mode "offlineCached", entitlements present).
  - `offline_wrong_password_bumps_and_errors` (Offline + wrong pw → Err + `failed_attempts == 1`).
  - `server_rejection_never_falls_back` — mock returns `InvalidCredentials`; assert NO offline session even when a valid verifier exists (verify returns Err(InvalidCredentials), and `load_verifier` was never consulted / session stays signed-out).
  - `no_verifier_for_email` (Offline + unknown email → the no-verifier error).
  - `account_b_verifier_cannot_open_account_a` — enroll A and B; offline sign-in with A's email + B's password → error, no session; A's email + A's password → A's session (isolation).
  - `throttle_grows_and_resets` — 5 wrong then timing assertion + a success resets `failed_attempts`.

- [ ] **Step 2: Run, verify fail.**

- [ ] **Step 3: Implement** the `sign_in` match + `offline_sign_in` + optional `CloudError::OfflineNoVerifier` variant (in `error.rs`).

- [ ] **Step 4: Run, verify pass** + build. Run the full `... cargo test -p cleophis cloud` (or session/integration filters).

- [ ] **Step 5: Commit.**

```bash
git add src-tauri/src/cloud/session.rs src-tauri/src/cloud/error.rs src-tauri/src/cloud/integration_tests.rs
git commit -m "feat(offline-auth): offline sign-in fallback (server-authoritative, per-account, throttled)

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 6: Account removal

**Files:**
- Modify: `src-tauri/src/cloud/commands.rs` (a `remove_account_from_device` command), `src-tauri/src/cloud/session.rs` (helper), `src-tauri/src/main.rs` (register)
- Test: `session.rs`/`integration_tests.rs`

**Interfaces:**
- Consumes: `store::{delete_verifier, read_auth_cache}` + the auth-cache path; kpack `packs_dir`/convstore per-account paths for the optional wipe.
- Produces: `#[tauri::command] async fn remove_account_from_device(user_id: String, wipe_local_data: bool, app: AppHandle) -> Result<(), String>`.

**Implementation notes:** Resolve the target `user_id`. If it equals `current_user_id()`, `sign_out()` first. Always: `delete_verifier(user_id)` + delete `auth-cache/<user_id>.json`. If `wipe_local_data`: delete `packs/<user_id>/` and `conversations/<user_id>.db` (reuse the same account_dir_segment sanitizer used by A6; never delete outside the managed dirs). This is a destructive, outward-visible local action — it is user-initiated from the profile menu with a confirm dialog (Task 7).

- [ ] **Step 1: Write failing test** — enroll an account (verifier + auth-cache + a dummy `packs/<uid>/` file); call the removal helper with `wipe_local_data=false` → verifier + auth-cache gone, packs dir still present; again with `wipe_local_data=true` on a second account → packs dir gone too. Removing the active account signs out.

- [ ] **Step 2: Run, verify fail.**

- [ ] **Step 3: Implement** the command + helper + register in `main.rs`.

- [ ] **Step 4: Run, verify pass** + build.

- [ ] **Step 5: Commit.**

```bash
git add src-tauri/src/cloud/commands.rs src-tauri/src/cloud/session.rs src-tauri/src/main.rs src-tauri/src/cloud/integration_tests.rs
git commit -m "feat(offline-auth): remove-account-from-device (verifier + auth-cache + optional data wipe)

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 7: Front-end — consent, strength meter, offline errors, removal menu

**Files:**
- Modify: `src/app.js`, `src/index.html`, `src/styles.css`
- Verify: `node -c src/app.js`

**Interfaces:**
- Consumes commands: `check_password_strength`, `sign_in`, `sign_up`, `remove_account_from_device`; the `SessionInfo.mode === 'offlineCached'` state (already handled in the header).

**Implementation notes (no Rust):**
1. **Strength meter:** on the sign-up + change-password inputs, an `input` listener calls `invoke('check_password_strength', {password, email, nickname})` (debounced ~150 ms), renders a 4-segment meter + the first feedback line, and disables the submit button unless `result.ok`. Escape all feedback via `textContent`.
2. **Consent:** a one-time modal at first successful online auth (gate on a persisted flag — a `localStorage`/settings key or a backend flag): the verbatim consent copy (Global Constraints) + an **[Agree]** button. Record the flag so it never shows again. (Enrollment happens backend-side regardless; the modal is the disclosure + the promised reversal pointer.)
3. **Offline errors:** in the `sign_in` catch/flow, map the backend "no verifier" error → *"Sign in online once on this device first."* and the offline wrong-password → *"Incorrect password."* Successful offline sign-in already renders `offlineCached` (header shows "offline, using saved account data").
4. **Removal:** a "Remove account from this device" item in the profile menu → a confirm dialog with a "also delete this account's data on this device" checkbox → `invoke('remove_account_from_device', {userId, wipeLocalData})` → on success, sign-out UI state.

- [ ] **Step 1** Add the strength meter markup + styles (`index.html`, `styles.css`) and the `input`-listener + `check_password_strength` wiring in `app.js`; disable submit unless `ok`.
- [ ] **Step 2** Add the consent modal (markup + one-time flag + Agree handler).
- [ ] **Step 3** Add the offline-error message mapping in the sign-in flow.
- [ ] **Step 4** Add the profile-menu removal item + confirm dialog + invoke.
- [ ] **Step 5** `node -c src/app.js` → clean. Static self-check: submit disabled on weak pw; consent shows once; removal confirm wired.
- [ ] **Step 6: Commit.**

```bash
git add src/app.js src/index.html src/styles.css
git commit -m "feat(offline-auth): FE consent + strength meter + offline errors + account removal

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 8: Opus adversarial security review → MSI → validation

**Files:** none (review + build).

- [ ] **Step 1** Build the review package (`review-package <base> <head>`) for the whole `feat/offline-auth` branch and dispatch an **opus** reviewer, pointed specifically at the two seams: (1) the Offline-vs-any-server-*response* branch in `sign_in` — the fallback must be unreachable on any server reply (walk every `Err` arm); (2) per-account isolation — B's verifier cannot open A's session (verify the test proves it, not just error codes). Plus: no plaintext password logged/stored; constant-time verify; normalization applies only to the local verifier; removal deletes only the managed paths; strength gate cannot be bypassed to enroll a weak password.
- [ ] **Step 2** Apply any fixes (new commits), re-verify tests + build.
- [ ] **Step 3** Reuse the `msi-build` agent to build the MSI at the branch HEAD; verify via the log `BUILD_EXIT_CODE=0` + a fresh MSI at `target/release/bundle/msi/` (mtime > HEAD commit epoch).
- [ ] **Step 4** Hand the MSI to the user to validate: sign in online (verifier enrolls silently, consent shows once) → sign out → go offline → sign back in with the right password (opens offlineCached) → wrong password (throttled, never locks) → a second account offline → remove an account from the device (with/without data wipe) → weak password rejected at sign-up with the meter. Then open PR #20.

---

## Self-Review

**Spec coverage:** verifier (§3 → Task 1), normalization (§4 → Task 1/2), storage keyring+auth-cache (§3 → Task 2), strength (§3/§9 → Task 3), enrollment (§5 → Task 4), offline flow + invariant (§6/§2 → Task 5), throttle persisted (§7 → Task 2 fields + Task 5 logic), account removal (§8 → Task 6), FE consent/meter/errors/removal (§9 → Task 7), threat-model caveats (§2 → carried in review Task 8), testing incl. the two Opus seams (§10 → across tasks + Task 8). All spec sections map to a task.

**Placeholder scan:** no TBD/TODO; each Rust module has concrete test code + named interfaces; the FE task lists concrete affordances (meter, consent copy verbatim, error strings, removal). Integration edits into existing files reference functions by name (`sign_in`, `auth::sign_in_password`, `offline_cached_info`, `current_user_id`, `grace_expired`) that were verified present during design — the implementer reads the exact surrounding lines.

**Type consistency:** `StoredVerifier`, `AuthCacheEntry`, `StrengthResult` names + fields are used consistently across tasks; `save_verifier`/`load_verifier`/`delete_verifier`, `read_auth_cache`/`write_auth_cache`/`find_user_by_email`, `check_strength`, `enroll_verifier`, `offline_sign_in`, `remove_account_from_device` names are stable across the tasks that produce/consume them.
