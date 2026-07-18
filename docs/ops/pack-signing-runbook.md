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

## Current state (v1) — verify path only, no key pinned

`CURATOR_PUBLIC_KEY` is `None` in this build. That is deliberate and fail-closed:
until a real key is pinned (at the §2.6 sign-and-publish milestone), **no curated
pack verifies, so none mounts** — which is correct, because there is no signing
infrastructure yet and no curated packs exist. The verify code path is complete
and adversarially tested (wrong key, truncated/oversized sig, tampered-bytes,
missing sig, malleability via `verify_strict`); only the key + the server-side
signing remain for §2.6.

**Release guardrail (enforced by construction):** the test keypair used by the
verify tests is generated only inside `#[cfg(test)]` — it is compile-time
impossible to link into a release binary. A test key that verified in release
would be a repo-resident universal pack-forgery key. Never move a test key to
crate scope; the only crate-scope key is the pinned production `CURATOR_PUBLIC_KEY`.

## When §2.6 lands: generating and pinning the curator key

1. **Generate the keypair offline.** On an air-gapped or HSM-backed machine,
   generate an ed25519 keypair. The **private key never leaves** that
   environment (HSM slot, or an encrypted offline store). Treat it like a
   code-signing root — its compromise forges every curated pack.
2. **Pin the public key.** Replace `CURATOR_PUBLIC_KEY = None` in
   `crates/kpack-core/src/sign.rs` with `Some([32 pinned bytes])`. This is
   public by design and ships in the app (same class as
   `cloud/config.rs`'s publishable key). Landing it is an app release.
3. **Wire the app to inject it.** `src-tauri` passes
   `sign::curator_verifying_key()` into `Pack::mount`'s `LoadContext.curator_key`
   for CDN-sourced packs.

## Signing a pack (§2.6, future, in the signing environment)

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
`Pack::mount`'s in-mount signature check is defense-in-depth on top of that. This
pre-open gate is the required wiring when a real key is pinned; until then it is
latent (no key → nothing mounts).

## Key rotation

Rotating the curator key is a coordinated event: pin the new public key (app
release), re-sign all curated packs with the new private key, and republish.
Old-key packs stop verifying on the new app — intended.
