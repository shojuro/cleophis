# Release-config audit (Phase 5.2)

**Status: checklist only — nothing here has been run.** No release APK exists,
because signing is founder-serialized.

## Why this file exists before the work does

Every item below is verifiable **only on a signed release build**, and the first
signed release build happens in the same session as the founder's signing
wiring. If the checklist is improvised during that session it will be written by
someone under time pressure who is also doing a key ceremony — so it is written
now, cold, by the people who found the items.

That ordering is the point: **the earliest possible verification for these items
is also the least convenient moment to be designing their verification.**

## The standing rule these inherit

Config claims are checked **against the built artifact**, never against the
config that was meant to produce them. Phase 0 shipped 239 MB of Windows
binaries into the APK because `bundle.resources = {}` read as correct in the
config, in review, and in the diff — and was a no-op (RFC 7386: `{}` merges,
only `null` deletes). Read the APK.

`docs/superpowers/mobile-tools/verify-apk.py` automates the artifact-level
checks that already have a runner. Items marked **manual** do not yet.

---

## Checklist

### 1. R8 did not strip the JNI bridge — **the item most likely to fail**

- [ ] `Lcom/cleophis/app/NativeBridge;` present in the release APK's dex
- [ ] `describeDevice` present in the release APK's dex
- [ ] `[bridge] ok round-trip …` appears in logcat from the **release** build

**Why this is first.** `isMinifyEnabled = false` for debug and `true` for
release. R8 shrinks on reachability from Java/Kotlin and **cannot see a Rust
caller across JNI**, so `describeDevice` is dead code to it. The debug smoke
test proves the bridge works and says *nothing* about release — a debug-only
proof with a release-only failure mode, which is a distinct member of this
project's "check that cannot fail" family: it is split across **build types**
rather than across artifacts or measurements.

`app/proguard-cleophis.pro` adds `-keep class com.cleophis.app.NativeBridge { *; }`.
**That rule is reasoned, not verified.** The pre-existing wry rule
(`-keep class com.cleophis.app.* { native <methods>; }`) is *not* sufficient — it
preserves the class name but keeps only `native` members, so the method would be
stripped and the symptom would be `NoSuchMethodError` (pointing at the
signature) rather than `ClassNotFoundException` (pointing at packaging).

Read it out of the dex, not out of a gradle log:

```python
import zipfile
z = zipfile.ZipFile(APK)
for n in z.namelist():
    if n.startswith('classes') and n.endswith('.dex'):
        b = z.read(n)
        print(n, b'Lcom/cleophis/app/NativeBridge;' in b, b'describeDevice' in b)
```

Anything else reached only from Rust across JNI inherits this item. As of now
that is the whole of `NativeBridge`; the `ConnectivityManager` and `ACTION_SEND`
shims will join it, and **each new Kotlin entry point needs its own keep rule
and its own line here.**

### 2. Debug-only flags absent from release

- [ ] `debuggable` **absent/false** in the packaged manifest
- [ ] `usesCleartextTraffic` **absent** — Tauri injects it into the *debug*
      manifest for dev-server support; release must not carry it
- [ ] `isJniDebuggable` not set

Both are present and correct in debug builds, so a diff against a debug APK is
the wrong comparison — check the release manifest on its own terms.

### 3. ABI and payload

- [ ] `lib/` contains **`arm64-v8a` only** (release `abiFilters`; debug keeps all
      flavors deliberately)
- [ ] `assets/` contains **`tauri.conf.json` only** (the `{}`-vs-`null` trap)
- [ ] zip-entry accounting: Σ compressed vs file length within ~1 %

The last one catches the zipflinger orphan: a stranded copy of a large library
that no central-directory entry points at. Phase 0's 334 MiB APK became 658 MiB
this way **and installed and ran perfectly**, which is exactly why file length
alone cannot detect it. `build-android-apk.sh` deletes the prior output to
prevent it; this check confirms the prevention worked.

- [ ] Release `.so` is **stripped and optimized** — debug ships ~340 MB of
      unstripped `libcleophis_lib.so`, and a release APK anywhere near that size
      means the strip did not happen

### 4. Privacy and hardening invariants (§6)

- [ ] `allowBackup=false`
- [ ] `fullBackupContent` / `dataExtractionRules` reference the exclusion XMLs
- [ ] **The conversation DB is ABSENT FROM THE BACKUP SET** — the only question
      that matters, and deliberately *not* phrased as "the exclusion rules are
      present". A rule that is present and covers nothing is exactly the defect
      this item was created by (see below), so an item that checks for the rule
      would inherit the bug it exists to catch.

  Run it, don't read it:

  ```
  adb shell bmgr enable true
  adb shell bmgr backupnow com.cleophis.app
  adb shell dumpsys backup | grep -A20 com.cleophis.app
  ```

  With `allowBackup=false` the expected result is that the package is not
  eligible at all — which is a *pass*, and also means this run says nothing
  about whether the XML rules work. To test the rules themselves, flip
  `allowBackup` to true in a **throwaway local build** (never committed), run
  the above, and confirm the DB, `auth-cache/` and `cloud-cache.json` do not
  appear. That is the only configuration in which the second layer is
  observable, and an unobservable safety net is what this item is about.

  **Why the item exists.** Until Phase 3.2's design work, both XMLs excluded
  `file` / `database` / `sharedpref` / `external` — and **none of those is
  where our data lives.** Tauri's `app_data_dir()` on Android is
  `activity.dataDir`, which AOSP's `FullBackup.getDirectoryForCriteriaDomain`
  maps to the **`root`** domain; `file` is `getFilesDir()`, a *child* of it. So
  the belt-and-braces layer excluded four directories we do not use, while
  reading in review, in the diff, and in its own comment as though it excluded
  everything. `allowBackup="false"` was doing all the work alone. `root` and
  `device_root` are now excluded; the *schema* is verified against the AOSP
  parser (bare `<exclude domain="…"/>` is well-formed — `path` is optional —
  and an excluded directory prunes its whole subtree), but **effectiveness is
  only observable here.**

- [ ] The exclusion XMLs are present *in the packaged artifact* with the `root`
      lines intact — read out of the APK's compiled resources, not the source
      tree, per the standing rule
- [ ] package identity `com.cleophis.app`; desktop keeps `com.cleophis.desktop`
- [ ] permission list is exactly what is intended — no permission without a
      caller, and none missing (`ACCESS_NETWORK_STATE` lands with the metered
      download policy)
- [ ] FGS `specialUse` service present with its justification property

### 5. Signing

- [ ] Signed with the release key, not the debug key
- [ ] Certificate digest recorded in the ledger, full-length, **never retyped**
- [ ] `versionCode` / `versionName` correct and rendered as text in-app

### 6. Provenance for the run itself

- [ ] `.provenance` sidecar written by the build, with `porcelain_exit 0` at
      **both** PRE and POST

Per the standing policy: scripts emit provenance, agents do not report it. An
empty `git status --porcelain` from a command that **timed out** (exit 124) is
byte-identical to a clean tree, so only `porcelain_exit 0` licenses "clean".

---

## Recording the result

Results go in `verification-milestone-mobile-p1.md`, with digests full-length
and every claim attributed to the artifact it was read from. An item that could
not be checked is recorded as **not checked** — this project's failure mode is
the confident-looking record, not the incomplete one.
