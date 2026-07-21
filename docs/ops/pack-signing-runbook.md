# Pack signing runbook — curated `.kpack` trust

How curated knowledge packs are signed (future) and how the app decides to trust
one (now). Source of truth for the code this describes: `crates/kpack-core/src/sign.rs`
(the ed25519 verify path), `crates/kpack-core/src/manifest.rs` (`Pack::mount`,
`LoadContext.curator_key`).

## Trust model in five lines

1. A **curated** pack (`pack_tier = curated`) is a `.kpack` plus a **detached
   ed25519 signature** file (`<pack>.kpack.sig`) over the whole pack bytes.
2. The app pins ONE **curator public key** as a compiled-in constant
   (`sign::CURATOR_PUBLIC_KEY`); it verifies the signature with `verify_strict`
   (rejects malleable/small-order signatures) — the key is injected into
   `Pack::mount` via `LoadContext.curator_key`, never read from the pack.
3. A curated pack that fails verification (bad/missing sig, wrong key, tampered
   bytes) **refuses to mount** — fail-closed, plain-language reason.
4. **Personal** packs are unsigned; they mount only from the user's local pack
   directory (origin trust), never from the CDN path.
5. The private key **never ships** and never touches a build machine — it lives
   only in the signing environment.

## Current state — production key pinned (Task B5), signing pipeline still pending

`CURATOR_PUBLIC_KEY` was `None` (fail-closed: no curated pack could verify, so
none mounted) from the kpack milestone until Task B5 (2026-07-21), which
generated the production curator keypair and pinned its public half in
`crates/kpack-core/src/sign.rs`. The private key lives outside this repo with
the curator and never touched a build machine. No curated packs shipped
before B5, so pinning the key changed no existing pack's behavior — it only
means a correctly-signed curated pack can now verify. The verify code path is
complete and adversarially tested (wrong key, truncated/oversized sig,
tampered-bytes, missing sig, malleability via `verify_strict`); the
server-side signing pipeline (below) and the CDN pre-open wiring (see
"Verifying in the app" below) are still outstanding — no curated packs exist
yet and none can be served from a CDN until that wiring lands.

**Release guardrail (enforced by construction):** the test keypair used by the
verify tests is generated only inside `#[cfg(test)]` — it is compile-time
impossible to link into a release binary. A test key that verified in release
would be a repo-resident universal pack-forgery key. Never move a test key to
crate scope; the only crate-scope key is the pinned production `CURATOR_PUBLIC_KEY`.

## Generating and pinning the curator key (done — Task B5, 2026-07-21)

1. **Generate the keypair offline.** On an air-gapped or HSM-backed machine,
   generate an ed25519 keypair. The **private key never leaves** that
   environment (HSM slot, or an encrypted offline store). Treat it like a
   code-signing root — its compromise forges every curated pack.
2. **Pin the public key.** Replace `CURATOR_PUBLIC_KEY = None` in
   `crates/kpack-core/src/sign.rs` with `Some([32 pinned bytes])`. This is
   public by design and ships in the app (same class as
   `cloud/config.rs`'s publishable key). Landing it is an app release. — done
   in Task B5.
3. **Wire the app to inject it.** `src-tauri` passes
   `sign::curator_verifying_key()` into `Pack::mount`'s `LoadContext.curator_key`
   for CDN-sourced packs. — this call site already existed pre-B5 and now
   resolves to `Some` automatically; it does NOT by itself add the pre-open
   `verify_file` gate for a CDN download path (see "Verifying in the app"
   below), because no such CDN kpack download path exists in this codebase
   yet.

## Signing a pack (future, in the signing environment)

1. Build the finished `.kpack` (server pipeline, §2).
2. Sign its exact bytes with the curator private key → write the 64-byte
   detached signature to `<pack>.kpack.sig`.
3. Upload BOTH `<pack>.kpack` and `<pack>.kpack.sig` to the CDN and append the
   pack to the signed catalog the wrapper polls.

## Verifying in the app — verify BEFORE parse for CDN packs

Security-critical ordering (K3 review residual): opening a `.kpack` with SQLite
runs the bundled SQLite + `sqlite-vec` C code on the pack's bytes. For a
**CDN-sourced curated pack**, the wrapper MUST call `sign::verify_file(pack_path,
sig_path, &curator_key)` — a bytes-only gate that touches no SQLite — **before**
`Pack::open`/`Pack::mount`, so forged/untrusted bytes never reach the C parsers.
`Pack::mount`'s in-mount signature check is defense-in-depth on top of that.
Task B5 pinned the real key, so this pre-open gate is now **required, live
wiring** for any CDN kpack-download path, not a latent future concern — but as
of B5 that wiring (and the CDN download path itself) does not exist yet in
this codebase, so there is currently nothing exercising it either way. This
MUST land before any curated pack is served from a CDN.

## Key rotation

Rotating the curator key is a coordinated event: pin the new public key (app
release), re-sign all curated packs with the new private key, and republish.
Old-key packs stop verifying on the new app — intended.
