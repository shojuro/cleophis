# Dependency Advisories — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Every open Dependabot alert and every RUSTSEC advisory the `cargo-audit` / `npm-audit` CI jobs report is fixed or formally accepted with a written reason, so the advisory jobs turn green on `main`.

**Architecture:** Three steps. (1) Plain version bumps where a compatible release exists: `pulldown-cmark` 0.13.6, `rusqlite` 0.41.1 (brings `libsqlite3-sys` ≥ 0.39), `epub` 2.1.6 (moves its chain to `zip` 9), `rustls` 0.23.46, and `sharp` 0.35.5 in `tools/covers`. (2) `docx-rust` 0.1.11 is the only crate still pinning `zip` 4 and has no newer release (its git head still says `zip = "4"`), so it is vendored under `third_party/docx-rust` with its `zip` requirement raised to 9 — the repo already vendors `llama-cpp-sys-2` the same way through `[patch.crates-io]`. (3) `glib` 0.18 comes from Tauri 2.11.5's GTK stack (the latest stable Tauri), is Linux-desktop-only, and is accepted in a record with the reason; its alert is dismissed as tolerable risk pointing at that record.

**Tech Stack:** Cargo workspace (rustc 1.97.1), npm (`tools/covers`), `cargo-audit` (installed locally at `~/.cargo/bin/cargo-audit`), GitHub Dependabot API via `gh`.

**Spec:** the open alerts (`gh api repos/shojuro/cleophis/dependabot/alerts?state=open`) and the `security.yml` job logs on `main`; the pin rationales in `crates/kpack-core/Cargo.toml`.

## Global Constraints

- Worktree `~/cleophis-wt/deps-bump`, branch `chore/deps-advisories` (from `origin/main` 4aadcc9). Never `cd` into the original repository roots. Never bare `git stash`. No secrets in the repo.
- Gates (ruling V1 stands): `cargo test -p kpack-core` and `cargo test -p kpack-engine` on WSL (both crates build here); the Android check `RUSTFLAGS='-D warnings' cargo ndk -t arm64-v8a -P 24 check -p cleophis --all-targets --release` (from `src-tauri/`, after `source ../.superpowers/ndk-env.sh`); the Windows desktop suite `cargo test --locked -p cleophis -- --test-threads=1` in `C:\Users\JM505 Computers\dev\cleophis-wt-win\vision-p1` detached at the commit under test (via `/mnt/c/Windows/System32/WindowsPowerShell/v1.0/powershell.exe`; the Windows worktree is shared with the copy-pass branch — one user at a time); `~/.cargo/bin/cargo-audit audit` from the workspace root; `npm audit --package-lock-only --audit-level=high` in `tools/covers`; `npm test` at the root (unchanged, 1283).
- Pins stay pins: a crate pinned `=x.y.z` for a stated reason is re-pinned `=x.y.w`, and the comment beside it is updated, never loosened to a caret.
- A vendored crate keeps its upstream LICENSE, carries a `VENDORED.md` naming the upstream version, the upstream commit it was copied from, the reason, and every line changed (`third_party` is `-diff` in `.gitattributes`, so the record is the only readable diff).
- D-3: no bare `#![allow(dead_code)]`; cfg'd allows carry a rationale. Zero warnings on the Android check.
- Commit trailers (last lines, this order): `Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>` then `Claude-Session: https://claude.ai/code/session_01W9ZnszzonHJc3yCaT3d2qM`. Commit after every task.

## The alerts (2026-11-13)

| Alert | Package | Range → fix | Where it comes from | Plan |
|---|---|---|---|---|
| RUSTSEC-2026-0459 | libsqlite3-sys 0.38.1 | ≥ 0.39 | rusqlite 0.40.1 (`src-tauri`, `kpack-core`, `kpack-cli` dev) | Task 1: rusqlite 0.41.1 |
| RUSTSEC-2026-0414, -0433 | pulldown-cmark 0.13.4 | ≥ 0.13.6 | `kpack-core` pin `=0.13.4` | Task 1: re-pin `=0.13.6` |
| RUSTSEC-2026-0347/-0348/-0349, Dependabot low/medium | rustls 0.23.45 | ≥ 0.23.46 | transitive | Task 1: `cargo update -p rustls` |
| CVE-2026-115909 (high), -115906 (medium) | zip 3.0.0 | ≥ 9.0.0 | epub 2.1.5 (`kpack-core` pin `=2.1.5`) | Task 1: re-pin epub `=2.1.6` (zip ^9) |
| CVE-2026-115905 (critical), -115909, -115906 | zip 4.6.1 | ≥ 9.0.5 | docx-rust 0.1.11 (`kpack-core` pin `=0.1.11`; upstream head still `zip = "4"`) | Task 2: vendor + bump to zip 9 |
| GHSA (medium) | glib 0.18.5 | ≥ 0.20 | gtk 0.18 ← tauri 2.11.5 (latest stable), Linux desktop only | Task 3: accept, record, dismiss |
| 3 × high | sharp 0.33.5 (`tools/covers`) | ≥ 0.35.5 | the cover-art tool | Task 1: `sharp@^0.35.5` |

---

### Task 1: The compatible bumps

**Files:**
- Modify: `crates/kpack-core/Cargo.toml` (pulldown-cmark `=0.13.6`, epub `=2.1.6`, rusqlite `0.41.1`, comments), `src-tauri/Cargo.toml` (rusqlite `0.41.1`), `crates/kpack-cli/Cargo.toml` (rusqlite `0.41.1`), `Cargo.lock`, `tools/covers/package.json`, `tools/covers/package-lock.json`

- [ ] **Step 1: Edit the pins** exactly as listed; update the rationale comments beside `pulldown-cmark` and `epub` to name the advisory ids and the date. Then `cargo update -p pulldown-cmark -p epub -p rusqlite -p libsqlite3-sys -p rustls` and confirm with `grep -A1 '^name = "<crate>"$' Cargo.lock` that the lock holds pulldown-cmark 0.13.6, epub 2.1.6, rusqlite 0.41.1, libsqlite3-sys ≥ 0.39, rustls 0.23.46, and that `zip 3.0.0` is gone (`epub` now pulls zip 9.x). Do NOT touch `docx-rust` or its `zip 4.6.1` (Task 2).
- [ ] **Step 2: rusqlite 0.40 → 0.41 API.** Build `kpack-core`, `kpack-cli` and the desktop crate; fix any breakage mechanically (the 0.41 changelog is small; the likely spots are `Connection::open*`, `params!`, `Row::get` lifetimes). No behaviour change.
- [ ] **Step 3: sharp.** In `tools/covers`: `npm install --package-lock-only --ignore-scripts sharp@^0.35.5` (lockfile only; no binary download), then `npm audit --package-lock-only --audit-level=high` → `found 0 vulnerabilities`. Keep `package.json`'s other fields untouched.
- [ ] **Step 4: Gates** — `cargo test -p kpack-core`, `cargo test -p kpack-engine`, the Android check (`Finished`, zero warnings), the Windows suite (expect 459 passed), `~/.cargo/bin/cargo-audit audit` (expect ONLY the three zip 4.6.1 advisories to remain; paste the summary), `npm test` 1283.
- [ ] **Step 5: Commit** — `chore(deps): pulldown-cmark 0.13.6, rusqlite 0.41.1, epub 2.1.6, rustls 0.23.46, sharp 0.35.5 — the compatible advisory fixes`.

---

### Task 2: Vendor docx-rust on zip 9

**Files:**
- Create: `third_party/docx-rust/` (the crate from `~/.cargo/registry/src/index.crates.io-*/docx-rust-0.1.11/`, minus `.cargo_vcs_info.json`/`Cargo.toml.orig`), `third_party/docx-rust/VENDORED.md`
- Modify: `Cargo.toml` (`[patch.crates-io] docx-rust = { path = "third_party/docx-rust" }`), `third_party/docx-rust/Cargo.toml` (`zip = { version = "9", default-features = false, features = ["deflate"] }`; version stays `0.1.11` so the pin `=0.1.11` in `kpack-core` keeps matching), `third_party/docx-rust/src/docx.rs` and `src/error.rs` as the compiler demands, `Cargo.lock`

- [ ] **Step 1: Copy and patch.** docx-rust touches zip in about 14 lines: `ZipArchive::new`, `by_name`, `ZipWriter`, `SimpleFileOptions::default().compression_method(CompressionMethod::Deflated)`, `ZipError`. All exist in zip 9.0.5 (`zip::write::SimpleFileOptions`, `zip::read::ZipArchive`, `zip::result::ZipError`); fix signatures as the build reports. The `async` feature (async_zip) stays optional and off — we do not enable it.
- [ ] **Step 2: Lock.** `cargo update -p zip@4.6.1` must report the chain moving to 9.x; `grep -A1 '^name = "zip"$' Cargo.lock` shows a single zip ≥ 9.0.5. `cargo tree -i 'zip@9*'` shows both epub and docx-rust.
- [ ] **Step 3: VENDORED.md** — upstream `docx-rust 0.1.11` (crates.io, 2026-01-22; repo `cstkingkey/docx-rs`, head b3852d2 on 2026-08-27 still on zip 4), MIT; why (the zip 4 chain carries CVE-2026-115905/-115909/-115906 and upstream has no release on zip 9); the full list of changed lines with before/after; how to re-vendor. Confirm `.gitattributes` marks `third_party/** -diff` (it does for the sys crate; extend the pattern if it is crate-specific) and say so in the file.
- [ ] **Step 4: Gates** — `cargo test -p kpack-core` (the docx fixtures round-trip), `cargo test -p kpack-engine`, the Android check, the Windows suite, `cargo-audit audit` → only the glib advisory may remain (it is a Dependabot alert, not a RUSTSEC one — expect `cargo-audit` to report 0 vulnerabilities; paste the line), `npm test`.
- [ ] **Step 5: Commit** — `chore(deps): vendor docx-rust 0.1.11 on zip 9 (CVE-2026-115905/-115909/-115906)`.

---

### Task 3: The record, the accepted alert, the PR notes

**Files:**
- Create: `docs/superpowers/deps-advisories-2026-11.md`

- [ ] **Step 1: The record** — the alert table above with the outcome column filled (fixed in which commit, or accepted), the gate lines verbatim from Tasks 1–2, the `cargo-audit` and `npm audit` summaries, and the glib acceptance: `glib 0.18.5` ← `gtk 0.18` ← `tauri 2.11.5` (latest stable; Tauri 3 is alpha), used only by the Linux desktop build, which is not shipped; the CVE's reach; revisit when Tauri's stable line moves to gtk 0.20. State that Dependabot PRs #36 (rustls) and #40 (sharp) are superseded by this branch and should be closed after merge, and that #38/#39 (GitHub Actions bumps) are unrelated and left alone.
- [ ] **Step 2: Dismiss the glib alert** with `gh api -X PATCH repos/shojuro/cleophis/dependabot/alerts/<number> -f state=dismissed -f dismissed_reason=tolerable_risk -f dismissed_comment='Linux desktop only (gtk 0.18 via Tauri 2.11.5, latest stable); not shipped; see docs/superpowers/deps-advisories-2026-11.md'` — find `<number>` from the alerts listing; record the number in the record. Dismissing is reversible (reopen with `state=open`).
- [ ] **Step 3: Commit** — `docs(deps): the November 2026 advisory record — what was bumped, what was vendored, what is accepted`.
