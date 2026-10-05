# CI for cleophis and cleophas-triage: tests, secret scan, dependency audits

**Date:** 2026-10-05
**Status:** design approved by the founder (placement option 1); this spec is awaiting review
**Repos:** `shojuro/cleophis` (public) at `mobile/triage-p6` e2f2348, PR #34; `shojuro/cleophas-triage` (private) at `mvp/phase1i` 9795722, PR #1

## 1. Goal

On every pull request into `main` and every push to `main`, GitHub Actions runs:

- **Tests.** Each repo's existing suites, exactly as the founder runs them locally.
- **Hygiene.** A secret scan and dependency audits. These do not block merges at first.

CI here means continuous integration only. Nothing is built for release, nothing is signed, nothing is published, and no credential of any kind enters CI.

## 2. Non-goals

- No deployment or publishing: no pack or catalog signing, no B2 uploads, no edge-function deploys, no APK or MSI release builds.
- No repository secrets. Every job runs on the default read-only `GITHUB_TOKEN` alone.
- No live checks against Supabase, Stripe, B2 or RunPod.
- No change to existing trigger scopes. In particular, `mobile-check.yml` stays push-only on `mobile/**`, the scope it records as founder-decided.
- No Node, Python or Rust version upgrade. CI matches the local toolchain (see §4); upgrading is a follow-up done locally and in CI together.

## 3. Hard rules

These come from the 2026-10-04 security review's CI rules.

1. **Triggers.** Use `pull_request` (base `main`), `push` (`main`), `workflow_dispatch`, and for the security workflows a weekly `schedule`. Never use `pull_request_target`.
2. **Permissions.** Every workflow sets top-level `permissions: contents: read` and grants nothing more.
3. **Pinned actions.** Every `uses:` is pinned to a full 40-character commit SHA, with the version in a trailing comment. Only first-party `actions/*` are used, plus the already-present `nttld/setup-ndk`, which also gets SHA-pinned.
4. **No stored token.** Every `actions/checkout` sets `persist-credentials: false`.
5. **Pinned, verified tools.** Tools such as gitleaks and cargo-audit are installed as pinned release binaries, and their sha256 is checked before use. No third-party installer actions.
6. **Timeouts and cancellation.** Every job has a `timeout-minutes`. A `concurrency` group cancels superseded runs of the same PR.
7. **No secrets.** No workflow references `secrets.*` (beyond the implicit `GITHUB_TOKEN`).

## 4. Toolchains

These match the founder's machine, since no repo pins them today:

| Tool | Version | How CI installs it |
| --- | --- | --- |
| Node | 20.19.2 | `actions/setup-node` |
| Python | 3.10 | `actions/setup-python` |
| Rust | 1.97.1 | `rustup toolchain install 1.97.1 --profile minimal`, made the default; rustup is preinstalled on the runners |
| gitleaks | 8.30.1 | release tarball, sha256-verified |
| cargo-audit | 0.22.2 | release binary, sha256-verified |
| pip-audit | 2.10.1 | `pip` |

## 5. Evidence

From a CI simulation on 2026-10-05: fresh clones of both PR heads, no `work/`, a clean `uv` environment from `requirements.txt` plus pytest.

| Suite | Result | Time |
| --- | --- | --- |
| cleophis `node --test src/ probes/` | 1211/1211 pass, no `npm install` needed | 2m20s |
| cleophas-triage `node --test probes/` | 630 pass, 2 skipped, 0 fail | 12s |
| cleophas-triage pytest | 3528 pass, 232 skipped, **13 fail** | 10m27s |

**All 13 failures are environment gaps, not defects (§7).**

**Skips.**
- 232 tests skipped: 230 for gitignored `work/` artefacts or a model tokenizer that isn't cached, 2 for the missing `torch`. All are by design.
- Some tests also look for founder-machine worktree paths. They skip cleanly where those paths don't exist, so a real runner will skip a few more than the simulation did. The auto-mode safety check did not allow those paths to be masked locally.

**Rust.** Read-only analysis of the code at e2f2348 found that `cargo test -p cleophis` on a clean desktop checkout needs:
- **The four bundle resource directories.** `src-tauri/resources/{llama,embedders,pdfium,ocr}` must exist; empty is enough. tauri-build exits 1 on a missing bundle-resource path. Only Android escapes this, because it sets `resources: null`.
- **No model, DLL, pack or key.** Every test that reads a fetched resource is `#[ignore]`. The reference-pack check in `build.rs` runs only when `CLEOPHIS_VARIANT=triage`.
- **A toolchain.** MSVC, CMake and LLVM/libclang, because llama.cpp builds from source. The `windows-latest` image has all three.

## 6. Design

### 6.1 cleophas-triage (private)

**`.github/workflows/ci.yml`**
- **Job `node`** (ubuntu): checkout, set up Node, `node --test probes/`.
- **Job `pytest`** (ubuntu):
  - Check out this repo to `triage/`.
  - Read `.github/mobile-pin`, a single line holding a full 40-hex SHA. Validate it against `^[0-9a-f]{40}$` and fail loudly otherwise.
  - Check out the **public** `shojuro/cleophis` at that SHA to `mobile/`. Use `fetch-depth: 0`, because tests read pinned historical commits through `git show`. No token is needed.
  - Set up Python and run `pip install -r requirements.txt -r requirements-test.txt`.
  - Run `CLEOPHIS_MOBILE_ROOT=$GITHUB_WORKSPACE/mobile python -m pytest -q -rs --ignore=work -p no:cacheprovider` in `triage/`, under `set -o pipefail`, with the output tee'd to a file.
  - Write the SKIPPED reasons, grouped and counted, to `$GITHUB_STEP_SUMMARY`. Coverage that CI cannot reach must be visible rather than silent.

**`.github/mobile-pin`** holds the mobile commit this repo's tests are read against, initially e2f2348 (full SHA). It moves only in a deliberate triage commit, as the repo's other pins do.

**`.github/workflows/security.yml`** (non-blocking: job-level `continue-on-error: true`, with findings written to the job summary):
- **Job `gitleaks`:**
  - On a PR, it scans the PR's commits (`base..head`).
  - On a push, it scans the pushed range.
  - On the weekly schedule, it scans full history, `--all`.
  - Config: `.gitleaks.toml` (§6.3).
- **Job `pip-audit`:** `pip-audit -r requirements.txt -r requirements-test.txt`.
- **Schedule:** weekly, Monday 06:00 UTC.

**`requirements-test.txt`:** `pytest` and `jsonschema`. `jsonschema` is imported only by `pipeline/analysis/tests/test_device_journey.py` (lines 595 and 608), so it is a test dependency.

**Cost.** One `ci` run is about 12–15 runner-minutes; `security` is about 2. The private repo draws on the 2,000 free minutes a month, and the `concurrency` cancellation keeps re-pushes from multiplying that.

### 6.2 cleophis (public; runner minutes are free)

**`.github/workflows/ci.yml`**
- **Job `node`** (ubuntu): checkout, set up Node, `node --test src/ probes/`.

**`.github/workflows/rust.yml`**
- **Triggers:** the common ones, but path-filtered to `src-tauri/**`, `crates/**`, `third_party/**`, `Cargo.toml`, `Cargo.lock` and `.github/workflows/rust.yml`.
- **Job `desktop-tests`** (`windows-latest`):
  1. Check out, install Rust 1.97.1, and cache `~/.cargo/registry`, `~/.cargo/git` and `target/`, keyed on OS + `Cargo.lock`.
  2. Create `src-tauri/resources/{llama,embedders,pdfium,ocr}`.
  3. Set `LIBCLANG_PATH=C:\Program Files\LLVM\bin`.
  4. Run `cargo test --locked -p cleophis -- --test-threads=1`. One thread matches the founder's gate protocol and the documented mock-server flakiness.
  5. Timeout: 90 minutes, because the first run compiles llama.cpp cold.
- **Job `crates-tests`** (ubuntu):
  - Same toolchain and cache.
  - Runs `cargo test --locked -p kpack-core -p kpack-calc -p kpack-cli -p kpack-pdf -p kpack-engine -p kpack-embed`. This covers signing, verification, the document parsers and the reference-pack rebuild-equals-pin test.
  - Needs no Tauri system packages.

**`.github/workflows/security.yml`** (non-blocking, also weekly):
- **`gitleaks`:** the same pattern as triage.
- **`npm-audit`:** `npm audit --package-lock-only --audit-level=high` at the repo root and in `tools/covers`.
- **`pip-audit`:** on `tools/pipeline/requirements.txt`.
- **`cargo-audit`:** on `Cargo.lock` and `crates/kpack-engine/Cargo.lock`. It is expected to report the known rustls advisory until that bump lands separately.

**`.github/workflows/mobile-check.yml`** keeps its trigger and is hardened:
- `actions/checkout` and `nttld/setup-ndk` SHA-pinned;
- `persist-credentials: false`;
- `cargo install cargo-ndk --locked --version 4.1.2` (the founder's local version).

### 6.3 Shared

**`.gitleaks.toml`** (per repo):
- `[extend] useDefault = true`, plus custom rules for:
  - Backblaze B2 application keys (key IDs are identifiers, not secrets, and a 25-hex rule would be noisy);
  - RunPod API keys;
  - DeepSeek keys.
- Narrow allowlists, by path and exact regex, for the false positives the 2026-10-04 full-history scan classified. Cleophis:
  - the `cfg(test)` fixture passwords in `session.rs`;
  - sha256 digests in `tools/reference/build/titles.json`;
  - NHS slugs containing `api`.

  Triage:
  - tokenizer sha256s in adapter manifests;
  - cluster labels;
  - `*-keys.env` file names.
- **Acceptance:** zero findings over each repo's full history, and every custom rule fires on a synthetic positive.

**`.github/dependabot.yml`** (per repo): the `github-actions` ecosystem only, weekly, so the SHA pins receive update PRs.

### 6.4 Test-hygiene changes in cleophas-triage

These are the three causes of the 13 simulated failures. Each mirrors a convention the repo already uses.

| Tests | Cause | Change |
| --- | --- | --- |
| `test_device_journey.py`: 2 tests | `jsonschema` imported but not declared | Declare it in `requirements-test.txt` |
| `test_rows.py`: 3 tests; `test_assemble_v10.py`: 4 tests | They read gitignored `work/` files with no skip guard, unlike every sibling | Add `pytest.mark.skipif` guards on those files' existence, worded like the siblings' |
| `test_train_dpo_generic.py`: 4 tests | They import `torch` without the `importorskip` a sibling already uses | `pytest.importorskip("torch")` |

**Behaviour on the founder's machine is unchanged.** With `work/` and torch present, all of these tests still run. Before and after counts are recorded.

## 7. Rollout

1. **Implement** on `sdd/ci-basic` in each repo, in worktrees `~/cleophis-wt/ci-basic` and `~/cleophas-triage-wt/ci-basic`, branched from the PR heads.
2. **Local checks:**
   - `actionlint`, a pinned and verified binary, passes on every workflow.
   - A static check passes: every `uses:` is SHA-pinned, and there is no `secrets.`, no `pull_request_target` and no `persist-credentials: true`.
   - The re-run CI simulation is green, and the full local suites are unchanged.
3. **Review** each repo's change set (spec and quality), then fix rounds as needed.
4. **Fast-forward** `mobile/triage-p6` and `mvp/phase1i` to their reviewed `sdd/ci-basic` tips. **Pushing is the founder's go/no-go**, because it starts the first real CI runs on PRs #34 and #1.
5. **Watch the first runs** and fix forward on `sdd/ci-basic`: fast-forward, then push.

## 8. Acceptance criteria

- **PR #1 (triage):**
  - `ci / node` and `ci / pytest` are green.
  - The pytest job summary lists skip reasons and counts.
  - `security` runs and reports.
- **PR #34 (mobile):**
  - `ci / node`, `rust / desktop-tests` and `rust / crates-tests` are green.
  - `security` runs and reports.
  - The hardened `mobile-check.yml` passes actionlint and the §3 checker. On the push its `audits` job is expected to stay red at D-6 (A1 is red by design; pre-existing, 8/8 recent runs) and the cross-compile job is skipped, so the hardened cross-compile steps are verified statically only until the founder decides whether A1 should report NOT-CHECKED instead of failing, or whether `mobile-check` should stop needing `audits` (a founder decision outside this branch).
- **Static rules:** every workflow passes actionlint and the §3 static check.
- **Local suites unchanged:** triage pytest 3779 passed / 0 failed with `work/` present (`pytest.ini` deselects the one `slow` test by default), triage node 632/0, mobile node 1211/0.

## 9. Risks and first-run handling

| Risk | Handling |
| --- | --- |
| Windows Credential Manager unavailable to the hosted runner: some `cloud/session.rs` tests write the real OS keyring with `.expect` | If they fail for that reason alone, exclude exactly those tests in CI via `--skip` names, and record the reason in the workflow comment and the PR. Never weaken the tests themselves. |
| Cold Windows build of llama.cpp (15–20 min) | Cache `target/`; the 90-minute timeout covers the first run. |
| Mock-server test flakiness (`cloud::rest`, `cloud::session`) | `--test-threads=1`. A flake is re-run once and recorded; it is never silently retried in a loop. |
| A real runner skips more triage tests than the simulation did (founder-path fallbacks) | The skip summary makes the difference visible. Making those roots honour `CLEOPHIS_MOBILE_ROOT` is a follow-up. |
| Private-repo minutes | Concurrency cancellation, and triggers limited to PRs into `main` and pushes to `main`. |

## 10. Follow-ups (out of scope)

- **A ruleset on `main`** in both repos requiring the `ci` checks. This is a founder settings change, made after CI is green. Path-filtered `rust` jobs cannot be required checks.
- **Version upgrades.** Node 20 is past end of life and Python 3.10 reaches it this month; upgrade local and CI together.
- **GitHub security settings.** Turn on Dependabot alerts and security updates, and code scanning on the public repo.
- **More CI coverage.** Make the hard-coded worktree roots in triage tests honour `CLEOPHIS_MOBILE_ROOT`, so that CI runs them.
- **An optional real-artifact job.** Fetch the embedder and pdfium, then run the `#[ignore]` tests with `--ignored`. This first needs the founder's decision on hosting a 118 MB file in CI.
- **`mobile-check.yml` audits gate.** Its `audits` job fails at D-6 on every run because acceptance item A1 is red by design, so the cross-compile job never runs (`needs: audits`). The founder decides whether A1 should report NOT-CHECKED instead of failing, or whether `mobile-check` should stop needing `audits`.
- **Promote the audits to blocking** once they are green: the rustls bump, sharp, and pinned requirements.
