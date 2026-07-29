//! The platform seam for the two secrets Cleophis persists: the Supabase
//! refresh token, and the per-account offline-sign-in verifier.
//!
//! Six functions, one surface, two bodies. Desktop is the OS credential store
//! via `keyring` — moved here verbatim from `cloud/store.rs`, not rewritten.
//! Android is a legible platform error until Phase 3.2 lands the Kotlin
//! `SecureStore` (AndroidKeyStore AES-256-GCM) behind the JNI bridge proven in
//! the native chunk.
//!
//! # Why this module exists at all
//!
//! Before it, `keyring` was an unconditional dependency and these six
//! functions called it on every platform including Android — where **it has no
//! backend**. Checked in `keyring-3.6.3`'s source rather than inherited from
//! the plan, because the whole phase rests on it:
//!
//! - `lib.rs` ends its platform ladder with
//!   `#[cfg(not(any(target_os = "linux", "freebsd", "openbsd", "macos",
//!   "ios", "windows")))] pub use mock as default;`. `target_os = "android"`
//!   matches none of those, so Android resolves to `mock` — the
//!   `linux-native` feature is gated on `target_os = "linux"`, which Android
//!   is not.
//! - `mock::MockCredentialBuilder::persistence()` returns
//!   `CredentialPersistence::EntryOnly`, and its `build()` constructs a fresh
//!   `MockCredential` for every `Entry::new`.
//!
//! Put together, the Android behaviour was worse than the "silent in-memory
//! mock" this milestone recorded: `save_refresh_token` stored the token in an
//! `Entry` that is **dropped on the next line** and returned `Ok`, while
//! `load_refresh_token` — building its own fresh `Entry` — could only ever
//! return `None`. Not a store with the wrong lifetime; a write that reports
//! success and keeps nothing.
//!
//! # What changes on Android, and what deliberately does not
//!
//! Nothing observable, and that is the intended result of 3.1. A live session
//! already survived on `Session`'s own in-memory `refresh_token` (mid-session
//! rotation reads it from there, never from the keyring), and boot-time
//! `restore()` already got `None`. So a force-stop still logs the user out —
//! what changes is that it now does so **for a reason that is written down**,
//! in logcat, instead of behind a call that claimed to have saved something.
//! 3.2 is what changes the behaviour; 3.1 makes the gap honest and removes a
//! dependency that cannot work from the Android graph.
//!
//! # Why the split is `not(target_os = "android")` and not `desktop`
//!
//! `desktop`/`mobile` are Tauri's cfgs, and `mobile` covers iOS too — where
//! `keyring`'s `apple-native` feature is off, so iOS would land on the same
//! mock. The gate that matters here is "does this target have a working
//! `keyring` backend compiled in", which is exactly the set the `Cargo.toml`
//! target section names. Keeping the two in the same vocabulary means the
//! dependency and the code that uses it cannot disagree about who gets it —
//! the coupling has one home rather than two (the etiology under D-4).

// Compiled on EVERY platform, deliberately (decision D-3). Only Android has
// blobs on disk, so on desktop this module is dead code — and that is the
// price being paid on purpose: every rule in it (which file a secret lives in,
// what AAD binds it there, how the on-disk envelope is framed) fails
// **silently** when wrong, and the desktop suite is the only place in this
// project where anything actually executes. A cross-compile `check` proves it
// compiles, not that it is right, and nothing runs on Android until a founder
// device checkpoint.
//
// The `allow` states a true, permanent platform fact rather than hiding an
// unknown — desktop uses the OS keyring and has no file layout to derive —
// which is this phase's standing rule for warnings: resolve on the merits,
// never blanket-suppress.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod blob;

#[cfg(not(target_os = "android"))]
mod desktop;
#[cfg(not(target_os = "android"))]
pub use desktop::*;

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "android")]
pub use android::*;
