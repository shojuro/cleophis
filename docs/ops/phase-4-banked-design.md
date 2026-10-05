# Phase 4 — banked design (DEFERRED, do not start)

**Status: deferred by founder directive**, pending founder-side items (Play/H8
registration, two more test devices, the signing session). Phase 5 runs first.

This file exists so the reasoning worked out at the Phase-2/4 boundary is not
re-derived from scratch when Phase 4 unblocks. **It is a design record, not a
task list.** Nothing here has been built.

---

## The signing wiring — four hard constraints

Ratified by steering. Each names a failure, not a preference.

### 1. The properties file is untracked, and the ignore rule is verified against the FILE

`keystore.properties` (or equivalent) holds the keystore path and passphrase.
It must never enter the repo.

**Verify with `git check-ignore -v <the actual path>`, never by reading the
pattern.** This is a required line in the wiring's verification, not a habit.

The reason is this project's sharpest lesson, in its most expensive costume: **a
`.gitignore` pattern has no type-checker.** A pattern that reads correct and
matches nothing is invisible in review, invisible in the diff, and invisible
until the file is committed. Phase 2 produced two members of exactly that class
— backup-exclusion rules naming four domains our data was not in, and an R8
keep rule nobody could verify — and both had survived for phases. The
discriminator for how expensive such a mistake gets is *whether a machine reads
the artifact*. Here nothing does, unless we make it: `git check-ignore -v` is
the mechanical checker the format does not provide.

### 2. The keystore path points OUTSIDE every repo

Not in `cleophis`, not in `cleophis-mobile`, not in any worktree. A path inside
a repo is one `git add -A` from disaster even with a correct ignore rule, and
ignore rules do not apply to already-tracked files.

### 3. The passphrase is never defaulted, echoed, or logged

No default value, no fallback, no printing it in a build log, no
`println!("using passphrase {…}")` even at debug level. The founder holds it.

### 4. An absent properties file FAILS THE RELEASE BUILD LOUDLY

**Never a silent fallback to debug signing.** This is the constraint with
catastrophic consequences and the one most likely to be implemented wrongly,
because "fall back to something that works" is the instinct.

A release APK signed with the debug key **installs fine**, runs fine, and looks
correct in every way — and is then **unupdatable by the real key forever**,
because Android refuses an update signed by a different certificate. Users would
have to uninstall (losing all local data: conversations, models, credentials) to
move to a properly signed build. That is the same catastrophic class as losing
the key itself, reached by a fallback that felt helpful.

So: absent file → hard failure with a message naming the file and what it must
contain.

---

## Why the signing wiring and the first release build are ONE session

Recorded for the founder-facing framing, already in `phase-2-complete.md`:

R8 strips code it believes unused and **cannot see that our Kotlin is called
from Rust**. If it strips the wrong thing the symptom is *"signed out on every
launch"* — which looks like a bug in something else entirely. Debug builds set
`isMinifyEnabled = false`, so a green CP3 device session is fully compatible
with a shipped build that fails this way.

The first signed release build is therefore the earliest moment several checks
can run **at all**, which is why `docs/ops/release-config-audit.md` was written
cold and made runnable (`verify-apk.py` now checks all four Rust-only Kotlin
entry points against the shipped dex, and is demonstrated capable of failing).
The audit is the deliverable that makes that session short.

---

## What Phase 4 still has to design when it unblocks

Not solved here, listed so the scope is known:

- `app_update.rs` — reuse `catalog_dist`'s `get_capped` / `parse_and_verify` /
  curator key (promote to `pub(crate)`), per-file downgrade guard via the
  existing `read/write_highest_version` plus a second state file.
- **The APK `url` must pass `download_host_allowed`** (security review M1).
- Refusals surfaced in UI *and* a local log; `versionName` rendered as text.
- Install shim: `canRequestPackageInstalls` → settings intent when needed →
  FileProvider `ACTION_VIEW`. **Never silent.** `file_paths.xml` already has
  `<files-path name="app_updates" path="updates/">` for this.
- Commands: `check_app_update` (launch + daily JS timer), `download_app_update`,
  `install_app_update`.
- The install shim is a **fifth** Rust-only Kotlin entry point, so it inherits
  the R8 keep rule and a `KOTLIN_EXPECTED` entry in `verify-apk.py`.
