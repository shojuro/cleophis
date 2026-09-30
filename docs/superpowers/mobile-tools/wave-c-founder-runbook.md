# Wave C: the founder's runbook (Phase 1i, Task MC1)

This page takes the triage build from a signed pack to a phone run. Agents
did everything up to the founder's three gates: signing the pack, signing and
publishing catalog v11, and running the phone. Each gate below is one block
of commands. No key, token or signed URL appears here. The one secret input
is `tools/pipeline/.env`, which names `CURATOR_KEY_FILE` (and the B2 pairs)
and which no agent reads.

**Already done by the agent (Task MC1):**

- The pack was rebuilt from the committed corpus. It is byte-identical to the
  pinned pack: sha256 `5c7b2c98…1cff1853`, 16,773,120 bytes, content sha
  `df9429a1…97c2b8fe5`, version 2026.09.1. It sits unsigned in
  `src-tauri/resources/packs/` of the build checkout, this worktree and the
  desktop checkout.
- The build checkout exists: a clean detached worktree at a07d8b7 with
  `node_modules` and the Tauri-generated `TauriActivity.kt` in place.
- A default-variant APK was built there as the toolchain proof, and
  `verify-apk.py` passed on it.
- The triage build was run without the `.sig` and refused with the designed
  message.
- An unsigned v11 was built by `build_catalog.py` from the live v10. Its entries
  match the bundled pins except the adapter's size, which only the fetched
  file can give.

**Why the build checkout is a separate worktree.** The release registration
pins `app_build` to `a07d8b7cea8f5bb86633447056df65080c4dd4db`, and the build
stamps HEAD into every export row. An APK built from any later commit,
including the commit that added this page, reads NOT MEASURED. So build and
run the probe guard from the detached checkout, and read this page and the
checklist from `mobile/triage-p6`. At a07d8b7 the scripts are not executable,
so call them through `bash`.

```bash
W=/home/penguinzyue/cleophis-wt/p6-final-fix        # mobile/triage-p6: this page, the pipeline, the kit
B=/home/penguinzyue/cleophis-wt/mc1-build-a07d8b7   # detached at a07d8b7: the build and the probe guard
DESK="/mnt/c/Users/JM505 Computers/dev/cleophis"    # the desktop checkout: holds tools/pipeline/.env
PUBKEY=158cb99e9756e2e4d01d88b7ecfeb99a76547821ffe9f9f1985316c5451cd0c0   # kpack_core::sign::CURATOR_PUBLIC_KEY
DIST=https://cleophis-dist.s3.us-east-005.backblazeb2.com
APK=$HOME/cleophis-artifacts/cleophis-triage-r0-debug-a07d8b7.apk
```

## (i) Sign the pack, then verify it (FOUNDER)

`sign_pack.py` reads the `.env` that sits next to it. The worktree has none,
so link the desktop checkout's. The link is gitignored, and step (iii) ends by
removing it. Your `.env` must already carry
`CURATOR_KEY_FILE=<path to the curator key, outside the repo, mode 600>`.

```bash
ln -s "$DESK/tools/pipeline/.env" "$W/tools/pipeline/.env"
P=$B/src-tauri/resources/packs/reference-uk-v1.kpack
test "$(sha256sum "$P" | cut -c1-64)" = 5c7b2c98337118ecd8a6fbd07887a639be81371b4e325504997768b41cff1853 && echo "pack sha OK"
python3 "$W/tools/pipeline/sign_pack.py" --pack "$P"
python3 "$W/tools/pipeline/sign_pack.py" --pack "$P" --verify-with "$PUBKEY"
cp "$P.sig" "$W/src-tauri/resources/packs/"
cp "$P.sig" "$DESK/src-tauri/resources/packs/"
```

You should see `pack sha OK`, then `[sign] wrote …reference-uk-v1.kpack.sig (64 bytes)`,
then `[verify] … verifies against the given public key`. The desktop
checkout's branch does not gitignore `packs/`, so never `git add` there.

## (ii) Build the triage APK, then check it (agent or founder, no secret)

```bash
cd "$B"
test "$(git rev-parse HEAD)" = a07d8b7cea8f5bb86633447056df65080c4dd4db && echo "HEAD OK"
git status --porcelain            # only "?? node_modules" may show
ls src-tauri/resources/packs/     # reference-uk-v1.kpack and reference-uk-v1.kpack.sig
bash docs/superpowers/mobile-tools/build-android-apk.sh --variant=triage
```

The build must print `== app build: a07d8b7cea8f5bb86633447056df65080c4dd4db ==`
and `== tauri exit status: 0 ==`. It must not print `triage build refused` or
`triage build without resources/packs`. Its provenance file, printed at the
end, records the APK's sha256 and size. Then archive and verify it:

```bash
mkdir -p "$HOME/cleophis-artifacts"
cp "$B/src-tauri/gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk" "$APK"
python3 "$B/docs/superpowers/mobile-tools/verify-apk.py" "$APK" "$HOME/android-sdk-linux/build-tools/35.0.0/aapt2"
python3 - "$APK" "$B/src-tauri/resources/packs/reference-uk-v1.kpack" <<'PY'
import hashlib, os, sys, zipfile
apk, pack = sys.argv[1], open(sys.argv[2], "rb").read()
so = zipfile.ZipFile(apk).read("lib/arm64-v8a/libcleophis_lib.so")
size = os.path.getsize(apk)
print("apk sha256 ", hashlib.sha256(open(apk, "rb").read()).hexdigest())
print("apk bytes  ", size)
print("pack in .so", pack in so)
print(f"pack share  {len(pack) / size:.2%} of the APK ({len(pack):,} of {size:,} bytes)")
PY
```

`verify-apk.py` must end `VERDICT: PASS`, and `pack in .so` must be `True`.
Send the four printed values to the controller, who fills the build record
in `phone-journey.md`.

`check-mobile-build.sh` fails at a07d8b7 before it compiles anything. Its
acceptance-coverage step reports A1 (no CI workflow) and A5 (no `bmgr`
backup test) as asserted but unbuilt. That red predates wave C and says
nothing about this APK. MC1 ran the script's remaining steps on their own:
the engine cross-compiles and the alignment probe library is 16 KB aligned
(`0x4000`). The shipped `libcleophis_lib.so` is linked 4 KB aligned (`0x1000`),
which the probe does not catch. That cannot affect a phone with 4 KB pages,
such as the A22 on Android 13. It is recorded for the controller as a spec
H2 gap. To re-run the script (expect exactly the A1 and A5 FAIL lines):

```bash
cd "$B"
ANDROID_NDK_ROOT=$HOME/android-ndk-r27c LIBCLANG_PATH=$HOME/.local/lib/python3.10/site-packages/clang/native \
  CARGO_TARGET_DIR=$HOME/cleophis-mobile-target bash docs/superpowers/mobile-tools/check-mobile-build.sh
git checkout -- docs/superpowers/mobile-tools/aarch64-align-probe/Cargo.lock   # the probe build rewrites it
```

## (iii) Sign and publish catalog v11, then verify it (FOUNDER)

Run from the worktree's pipeline directory. System `python3` has `boto3` and
`cryptography`, so no venv is needed.

1. Fetch and check the two served files. Run steps 1 to 3 of
   `docs/superpowers/mobile-tools/publish-served-form.md` from this directory,
   using `python3` where it says `./.venv/bin/python3`. They fetch from
   `cleophis-models`, require five `OK` lines, and write both manifests. The
   adapter manifest takes its size from the fetched file.
2. Build, check, sign, publish and verify:

```bash
cd "$W/tools/pipeline"
mkdir -p work/live
curl -fsS "$DIST/catalog.json"     -o work/live/catalog.json
curl -fsS "$DIST/catalog.json.sig" -o work/live/catalog.json.sig
python3 build_catalog.py \
  --manifests work/out/Qwen3-1.7B/Q6_K/manifest.json work/out/Qwen3-1.7B/adapter/manifest.json \
  --base-url "$DIST" --carry-forward work/live/catalog.json --carry-forward-pubkey "$PUBKEY"
python3 - "$B/src-tauri/resources/catalog.triage.json" <<'PY'
import json, os, sys
old = json.load(open("work/live/catalog.json"))["artifacts"]
new = json.load(open("work/catalog/catalog.json"))
e = json.load(open(sys.argv[1]))[0]
assert new["catalog_version"] == 11 and new["artifacts"][:11] == old, "carry-forward mismatch"
b, a = new["artifacts"][11:]
assert (b["kind"], b["sha256"], b["size"]) == ("base", e["sha256"], e["fileBytes"]), b
assert (a["kind"], a["sha256"]) == ("adapter", e["adapterSha256"]), a
assert a["size"] == os.path.getsize("work/out/Qwen3-1.7B/adapter/triage-v3-Qwen3-1.7B.gguf"), a
assert os.path.basename(b["path"]) == os.path.basename(e["modelFile"])
assert os.path.basename(a["path"]) == os.path.basename(e["adapterFile"])
print("v11 matches the bundled catalog.triage.json pins")
PY
python3 sign_catalog.py --catalog work/catalog/catalog.json
python3 sign_catalog.py --catalog work/catalog/catalog.json --verify-with "$PUBKEY"
python3 publish.py --carried-forward work/live/catalog.json --verify-pubkey "$PUBKEY" --dry-run
python3 publish.py --carried-forward work/live/catalog.json --verify-pubkey "$PUBKEY"
python3 verify_published.py --pubkey "$PUBKEY"      # re-downloads every artefact, about 12 GB
sha256sum work/catalog/catalog.json                 # send this to the controller
rm "$W/tools/pipeline/.env"                         # removes the link only
```

The build must report `carrying 11 artifact(s) from catalog_version=10` and
`catalog_version=11, 13 artifact(s)`. `verify_published.py` must end with an
`OK:` line.

## (iv) Install the APK (FOUNDER)

Pair the phone over wireless debugging (see the header of
`run-on-device.sh`), then:

```bash
adb devices
adb install -r "$APK"
adb shell pm path com.cleophis.app
```

## (v) Keep the laptop checkout clean at the build commit

The probe guard reads the checkout it runs from. It must be `$B`, at
a07d8b7, with nothing changed under `src/triage/`. This must print nothing:

```bash
git -C "$B" status --porcelain src/triage/
```

## (vi) Prepare the hand-back folder, the inputs and the keyboard

```bash
cd "$HOME"
bash "$W/docs/superpowers/mobile-tools/phone-run-kit.sh" prepare "$HOME/device-journey-r0"
```

The kit makes `exports/`, `exports-after-restart/`, `probe/` and
`screenshots/`, and a `founder-steps.json` listing all 21 steps with empty
results. It pushes `phone-journey-inputs.txt` to the phone's Downloads. Then,
on the phone's keyboard, turn off auto-capitalisation, autocorrect and smart
punctuation. The kit prints where those settings live. Paste every input.

## (vii) Run the checklist

Follow `$W/docs/superpowers/mobile-tools/phone-journey.md` from S1 to C1.
Record each step in `founder-steps.json` as `"PASS"` or `"FAIL"`. Anything
else reads as NOT RECORDED. Run the harness and the device probes through
`bash` from `$B`:

```bash
cd "$B"
bash docs/superpowers/mobile-tools/airplane-mode.sh --interactive 1800                     # S4
bash docs/superpowers/mobile-tools/run-device-probes.sh \
  --prompts "$HOME/device-journey-r0/probe/release-r0.jsonl" --out "$HOME/device-journey-r0/probe"
node probes/device-guard.mjs --in "$HOME/device-journey-r0/probe/release-r0.<serial>.json" \
  --out "$HOME/device-journey-r0/probe/release-r0.guard.jsonl"
```

The prompt set `release-r0.jsonl` comes from the triage repository's
`device_probes` command, as the checklist's device-probe section says. The
controller supplies its `<gate>` and `<shipped stack>` values.

## (viii) Export and hand back

```bash
bash "$W/docs/superpowers/mobile-tools/phone-run-kit.sh" pull        "$HOME/device-journey-r0"   # E1
bash "$W/docs/superpowers/mobile-tools/phone-run-kit.sh" pull-replay "$HOME/device-journey-r0"   # R1
bash "$W/docs/superpowers/mobile-tools/phone-run-kit.sh" pack        "$HOME/device-journey-r0"
```

Delete any pulled log that is not from this journey before `pack`. Then tell
the controller where `device-journey-r0.zip` is. The reader command is the
last block of `phone-journey.md`.

## What the controller does next

1. Fill the build record in `phone-journey.md` from the values sent in (ii)
   and (iii), and commit it on `mobile/triage-p6`.
2. Commit the post-publish fill amendment to the release registration. It
   sets the catalog dist v11 sha, which TC2 left `null` as "FILLED BY
   AMENDMENT AFTER PUBLISH". It adds the APK sha as provenance.
3. Run the device-journey reader on the hand-back and then the release gate,
   Task TC2b. The phone ships the shipped adapter v3 only if the gate passes.
