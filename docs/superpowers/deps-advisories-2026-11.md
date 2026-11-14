# Dependency advisories, November 2026

Record of what the `chore/deps-advisories` branch fixed, vendored and accepted.
Plan: `docs/superpowers/plans/2026-11-13-deps-advisories.md`. Alerts read on 2026-11-13
(Dependabot alerts plus the `security.yml` job logs on `main`, 4aadcc9).

## The alerts and their outcome

Alert numbers are the live Dependabot alerts read on 2026-11-13; all eleven are open and each
maps to a row. After this branch merges, every alert except glib (#1) should close on
Dependabot's next scan. Alert #1 is the founder's dismissal (below).

| Dependabot alert # | Advisory | Package | Range, fix | Outcome |
|---|---|---|---|---|
| none (cargo-audit only) | RUSTSEC-2026-0459 (CVE-2026-119375) | libsqlite3-sys 0.38.1 | needs 0.39 or later | **Accepted** (see below). Held at rusqlite 0.40.1 / libsqlite3-sys 0.38.1; ignored in `.cargo/audit.toml`. |
| none (cargo-audit only) | RUSTSEC-2026-0414, -0433 | pulldown-cmark 0.13.4 | 0.13.6 or later | **Fixed** in cca47b7: 0.13.4 to 0.13.6 (`kpack-core` pin `=0.13.6`). |
| #4, #5, #6, #8 | RUSTSEC-2026-0346 (medium, #4 GHSA-c8m5-c952-6cm4), -0347 (#8), -0348 (#6), -0349 (#5) | rustls 0.23.45 | 0.23.46 or later | **Fixed** in cca47b7: 0.23.45 to 0.23.46. |
| none (cargo-audit only) | RUSTSEC-2026-0382 | rustls-webpki 0.103.15 | 0.103.17 | **Fixed** in 031b451: 0.103.15 to 0.103.17 (ruling DA-4). Found by `cargo-audit` after the first bump. |
| none (cargo-audit only) | RUSTSEC-2026-0381 | webpki-roots 1.0.8 | 1.0.12 | **Fixed** in 031b451: 1.0.8 to 1.0.12 (ruling DA-4). The 0.26.11 line on the ureq side is a shim over 1.0.12. |
| #7, #9, #10 | CVE-2026-115905 (critical), CVE-2026-115909 (high), CVE-2026-115906 (medium) | zip 3.0.0 | 9.0.5 or later | **Fixed** in cca47b7: epub 2.1.5 to 2.1.6 moves the chain to zip 9.5.1; zip 3.0.0 is gone from the lock. |
| #7, #9, #10 | CVE-2026-115905 (critical), -115909, -115906 | zip 4.6.1 | 9.0.5 or later | **Vendored** in 4da54e7: docx-rust 0.1.11 on zip 9. The same three alerts cover zip 3 and zip 4; they clear once neither remains. |
| #1 | GHSA-wrw7-89jp-8q8g (medium) | glib 0.18.5 | 0.20 or later | **Accepted** (see below). Dismissal is the founder's command, printed below. |
| #2, #3, #11 | GHSA-f88m-g3jw-g9cj, GHSA-rgj7-g3m4-5g8c, GHSA-wq5f-xc86-pv6w (high) | sharp 0.33.5 (`tools/covers`) | 0.35.5 or later | **Fixed** in cca47b7: `sharp@^0.35.5`; `npm audit` reports 0. |

### The two id schemes for zip

Dependabot reports the zip findings under CVE ids (CVE-2026-115905, -115909, -115906).
`cargo-audit` reports the same crate under four RUSTSEC ids: RUSTSEC-2026-0367, -0368, -0369
and -0429. Both lists describe the zip 4.6.1 chain that the vendoring removes (the same CVEs also covered zip 3.0.0, which epub's bump had already removed). The webpki pair
is likewise RUSTSEC-2026-0382 (rustls-webpki) and RUSTSEC-2026-0381 (webpki-roots), found by
`cargo-audit` rather than by the plan's Dependabot listing.

## Accepted: RUSTSEC-2026-0459 (libsqlite3-sys, SQLite below 3.54)

Ruling DA-3. CVE-2026-119375.

**What the fix costs.** The fix is libsqlite3-sys 0.39, which bundles SQLite 3.54, reached
through rusqlite 0.41. Moving to it changes the bytes of the curator-signed, pinned triage
pack. kpack-cli's `committed_corpus_rebuilds_byte_identically_and_reproduces_the_contract_files`
failed on cca47b7: the rebuild produced sha f607de1f... instead of the pinned 5c7b2c98...
(full pin `5c7b2c98337118ecd8a6fbd07887a639be81371b4e325504997768b41cff1853`). It passes again
on 031b451, which holds SQLite back. A triage build fails unless the embedded pack equals
that sha, and the pack is signed with the curator key, so rebuilding it is a founder decision.

**Exposure.** The advisory needs a database opened together with a `-wal` file that a
less-trusted party wrote. Every database in this app is app-private, and the signed packs
ship without WAL siblings. Exposure is low.

**Where the acceptance lives.** `.cargo/audit.toml` carries the one ignore, with the reason as
its comment.

**Follow-up (founder, curator key).**

1. Rebuild and re-sign the triage pack on SQLite 3.54 or later.
2. Re-pin the new hash in every home. `grep -rl 5c7b2c98 --include=*.rs --include=*.json --include=*.md`
   found these nine files on this branch:
   - `crates/kpack-cli/tests/reference_pack.rs` (`PACK_SHA256`)
   - `src-tauri/build_checks.rs`
   - `src-tauri/src/catalog.rs`
   - `src-tauri/src/convstore.rs`
   - `src-tauri/resources/catalog.triage.json`
   - `docs/superpowers/specs/2026-09-09-med-triage-mvp-phases-0-3.md`
   - `docs/superpowers/mobile-tools/build-reference-pack.md`
   - `docs/superpowers/mobile-tools/phone-journey.md`
   - `docs/superpowers/mobile-tools/wave-c-founder-runbook.md`
3. Delete the ignore from `.cargo/audit.toml`.
4. Take rusqlite 0.41 (libsqlite3-sys 0.39 or later) in `src-tauri`, `kpack-core` and
   `kpack-cli`, one version everywhere.

## Accepted: glib 0.18.5

Ruling DA-1. GHSA-wrw7-89jp-8q8g, "Unsoundness in `Iterator` and `DoubleEndedIterator` impls
for `glib::VariantStrIter`". Dependabot alert **#1**.

`glib 0.18.5` comes from `gtk 0.18`, which Tauri 2.11.5 pulls in. The current stable Tauri,
2.12.5 (checked on crates.io on 2026-11-13), still depends on `gtk ^0.18`, so no stable Tauri
bump fixes it. Tauri 3 is alpha. The GTK stack is used only by the Linux desktop build, which
is not shipped. Revisit when Tauri's stable line moves to gtk 0.20 (glib 0.20 or later).

The dismissal is **not** run by this branch. It writes to GitHub, so it is the founder's
command (ruling DA-5):

```
gh api -X PATCH repos/shojuro/cleophis/dependabot/alerts/1 -f state=dismissed -f dismissed_reason=tolerable_risk -f dismissed_comment='Linux desktop only (GTK 0.18 under Tauri 2.11.5; current stable Tauri 2.12.5 still depends on gtk 0.18); not shipped; see docs/superpowers/deps-advisories-2026-11.md'
```

Reopen, if the decision changes:

```
gh api -X PATCH repos/shojuro/cleophis/dependabot/alerts/1 -f state=open
```

## Vendored: docx-rust 0.1.11 on zip 9

Ruling DA-2. Full record, with every changed line before and after, in
`third_party/docx-rust/VENDORED.md`. Summary:

- docx-rust 0.1.11 requires `zip = "4"` and upstream has no release on zip 9. It is vendored
  under `third_party/docx-rust` and wired in by `[patch.crates-io]` in the root `Cargo.toml`.
  The version stays 0.1.11 so the `=0.1.11` pin in `kpack-core` still matches.
- Three source edits: the `zip` requirement in the vendored `Cargo.toml` from 4 to 9; the two
  `file_names()` calls in `src/docx.rs` (lines 471 and 489); one lifetime in
  `src/document/numbering.rs` (line 163) that `-D warnings` demands.
- **Behaviour note.** zip 9's `file_names()` yields `ZipResult<Cow<str>>`. The two calls now use
  `filter_map(ok)`, so an entry name zip cannot decode is skipped rather than listed. This
  affects only the two multi-file readers (`option_read_multiple`, `option_read_multiple_files`).
  All 218 kpack-core fixtures still round-trip.
- Attributes: `third_party/.gitattributes` already applies `* -text -diff` to the whole tree.
  The nested `third_party/docx-rust/.gitattributes` (`VENDORED.md diff`, `LICENSE diff`,
  `README.md diff`) only makes the human-readable files diffable. `VENDORED.md` is the readable
  record of every difference from the registry crate.
- The vendored tree keeps only what we build: 113 tracked files (108 under `src/`, plus
  `Cargo.toml`, `LICENSE`, `README.md`, `VENDORED.md`, `.gitattributes`). Upstream `tests/`,
  `examples/`, `validator/`, `.github/`, `.editorconfig`, the upstream `.gitignore` and
  `Cargo.lock` are dropped, and `VENDORED.md` names each omission, including the `*.docx`
  files upstream's own `.gitignore` had excluded.
- Lock facts after the vendoring: `grep -A1 '^name = "zip"$' Cargo.lock` prints one entry,
  `zip` version `9.5.1`. No `arbitrary` or `derive_arbitrary` entry remains in the lock, and no
  entry names zip 4. `cargo-audit audit -f Cargo.lock` reports no vulnerabilities, and that
  report is the authority.
- Retire the vendoring once upstream releases on zip 9 or later.

## Gates

Verbatim from the final round of each task.

Task 1, round 2, commit 031b451:

```
cargo test -p kpack-core:   ok. 218 passed; 0 failed; 1 ignored
cargo test -p kpack-engine: ok. 166 passed + 4 passed; 0 failed
cargo test -p kpack-cli:    committed_corpus_rebuilds_byte_identically_and_reproduces_the_contract_files ... ok
                            test result: ok. 9 passed; 0 failed; finished in 600.74s
Android check:              Finished `release` profile [optimized] target(s) in 13m 26s (zero warnings, -D warnings)
Windows suite on 031b451:   test result: ok. 459 passed; 0 failed; 10 ignored; == exit 0 ==
cargo-audit -f Cargo.lock:  error: 4 vulnerabilities found!  (zip 4.6.1: RUSTSEC-2026-0367, 0368, 0369, 0429)
                            warning: 14 allowed warnings found
cargo-audit -f crates/kpack-engine/Cargo.lock: no vulnerabilities, exit 0
npm test:                   # pass 1283, # fail 0
npm audit --package-lock-only --audit-level=high (tools/covers): found 0 vulnerabilities
```

Task 2, commit 4da54e7:

```
cargo test -p kpack-core:   218 passed; 0 failed; 1 ignored
cargo test -p kpack-engine: 166 passed + 4 passed
cargo test -p kpack-cli:    4 passed, 12 passed, 9 passed; 0 failed ... finished in 473.81s
Android check:              Finished release profile [optimized] target(s) in 1m 21s (-D warnings)
Windows suite on 4da54e7:   test result: ok. 459 passed; 0 failed; 10 ignored
cargo-audit -f Cargo.lock:  exit 0, no vulnerabilities, "14 allowed warnings found"
                            (sqlite RUSTSEC-2026-0459 ignored via .cargo/audit.toml)
cargo-audit -f crates/kpack-engine/Cargo.lock: exit 0, clean
npm test:                   tests 1283, pass 1283, fail 0
```

The Windows exit line for 4da54e7 was lost to PowerShell quoting; the result is read from the
log's test-result lines. The `npm audit` result for `tools/covers` was not re-run in Task 2,
which does not touch it.

## History note

The message of cca47b7 names rusqlite 0.41.1. That commit did take rusqlite 0.41.1 and
libsqlite3-sys 0.39.1. Commit 031b451 reverted to rusqlite 0.40.1 / libsqlite3-sys 0.38.1 for
the reason above. Both commits are kept; the net manifest change for rusqlite is none.

## Dependabot pull requests

- #36 (rustls) and #40 (sharp) are superseded by this branch. Close them after it merges.
- #38 and #39 (GitHub Actions bumps) are unrelated and left alone.

## Traps

- `cargo update --precise 0.40.1` for rusqlite first landed rusqlite 0.40.2 and libsqlite3-sys
  0.38.2. Each crate had to be pinned explicitly to land 0.40.1 and 0.38.1.
- The audit ignore prints no "ignored" line in this cargo-audit version. The proof it works is
  the count: 4 vulnerabilities with `.cargo/audit.toml`, 5 without it (the fifth is
  RUSTSEC-2026-0459).
- After the vendoring, the zip 4.6.1 entries that Task 1's audit counted are gone from the
  workspace lock, and the ignore is the only exception in force.
