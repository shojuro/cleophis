# Handoff — the native chunk (JNI bridge, then download policy + share sheet)

Written at the 2.2/native boundary by the fourth-generation `mobile-p1`
instance, which stopped here deliberately rather than open a large native chunk
near its context limit. Gen-1 died silently; gen-2 and gen-3 stood down after
writing handoffs. This continues that.

**State at writing:** 2.2's layout, engine-state presentation, D-4 and
interrupted-download visibility are DONE and gated. Desktop gate GREEN at
`0ede91b` (322/0). `npm test` 58/58. Tree clean.

---

## Read these first

1. `verification-milestone-mobile-p1.md` — the 2.2 section, decisions D-4 and
   D-5, and the coupling etiology. The last is the one to internalise: it is
   the sharpest lesson this phase produced and it generalises well beyond the
   number it was about.
2. `specs/2026-07-24-mobile-p1-brief.md` — standing orders.
3. `mobile-tools/layout-harness.md` — how to measure the frontend without a
   device, and (as prominently) what it cannot tell you.

---

## The task: bridge FIRST, as its own commit

Steering ratified this sequencing on the strength of a read-only audit. The
facts, so you do not have to re-derive them:

- **The JNI bridge has zero prior art in this repo.** `jni = "0.21"` is
  declared under `[target.'cfg(target_os = "android")'.dependencies]`, and
  **nothing** in `src-tauri/src/` references `jni::`, `JNIEnv`, or
  `ndk_context`. Grep it yourself before believing me — that is the habit this
  track runs on.
- **`ndk_context` is not declared at all.** It is what yields the `JavaVM` and
  the `Context` from the Android runtime under Tauri's activity. You start one
  dependency short of what the brief's design assumes.

**Deliverable: one commit that adds `ndk_context`, acquires the `JavaVM` and
`Context`, makes ONE honest round-trip call into Kotlin and back, and proves it
on the device.** Not a feature. A round trip whose only job is to fail loudly
if the bridge does not work.

**Why its own commit and its own checkpoint.** Three separate shim surfaces
(`ConnectivityManager`, `ACTION_SEND`, SAF/content-URIs) all sit behind this
one bridge. If the first of them is built on an unproven bridge, a failure has
two candidate causes and one symptom — compound debugging. This track has
already paid that bill once at full price: the gradle launcher bug surfaced as
`Execution failed for task ':app:rustBuildArm64Debug'`, a *Rust* task, after
the Rust build had genuinely succeeded, and chasing the Rust build is what
plausibly consumed an entire agent's context.

**Predict what the smoke test will print, then run it.** A round trip that
returns a value you did not predict is not a passing bridge.

---

## Then, in order

1. **`ConnectivityManager` → download network policy.** Wi-Fi/unmetered-only by
   default, per-download override, charge-recommended notice.
   **`navigator.connection` is NOT a substitute** — it reports wifi-vs-cellular,
   and a metered Wi-Fi hotspot is exactly the case that distinction gets wrong.
   Do not build the policy on it; the whole point of the policy is respecting a
   user's data plan, and a false-negative there costs them money.
2. **`ACTION_SEND` → share-sheet export (MD/TXT/JSON).** Note `exportChat` in
   `app.js` currently uses `dialog.save`, which is a desktop file-picker
   assumption. The mobile path is a share sheet, and `export_chat_to_file`
   already produces the bytes — you need the destination, not the formatting.
3. **Only then** anything for the founder's pack-upload feature, which is
   logged as future work and explicitly NOT scheduled.

---

## Things that will bite you if nobody says them

- **The frontend is embedded into `libcleophis_lib.so` at RUST COMPILE TIME.**
  `assets/` in the APK holds only `tauri.conf.json` — no html/css/js. So
  editing `src/` after that compile yields an APK matching no commit. **Freeze
  the tree for your own builds, not only for steering's gate runs.** I had to
  discard a build for exactly this.
- **`keyring` is still ungated** (unconditional, `windows-native` +
  `linux-native`, neither applying on Android), so v3's silent in-memory mock
  is live and **a force-stop still logs the user out on device**. This is the
  single most likely thing to look like a serious bug in a founder session
  while being entirely expected. It is 3.1's to fix.
- **`cargo ndk … check --all-targets` does NOT compile the desktop test
  modules** — they are `#[cfg(all(test, desktop))]`. A Rust test you add has
  never been built until the Windows gate runs it. Mine passed, but it went to
  the gate with zero prior execution, and I said so rather than discovering it
  as a surprise.
- **`git status` on this drvfs worktree can take minutes.** Give it a generous
  timeout, and never read an empty result from a command that timed out as
  "clean" — I made that exact mistake and had to walk it back.
- **A trailing `&` applies to an entire `&&` chain.** I backgrounded a whole
  provenance-recording command that way and misread its truncated output as a
  clean tree. Record provenance in its own command.
- **Playwright MCP writes into `/mnt/c/.../dev/cleophis`** — the
  english-tutor worktree you must never touch. Move screenshots out and clear
  `.playwright-mcp/` after each run.

---

## The habit that actually found things this session

Two of the three real defects I fixed were found by **writing down why the
existing code was correct and discovering it was not** — the ledger already
calls this out twice and it earned its keep a third and fourth time. It caught
a comment of my own claiming a 4096 default was "a safe floor" when 4096 is too
*large* on a floor device.

And for anything with visual output: **render it and look.** Three defects this
session passed their assertions and were caught only by screenshotting. The
measurements confirm the property you thought to measure, never the one you
did not.
