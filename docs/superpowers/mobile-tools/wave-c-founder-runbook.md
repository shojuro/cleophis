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
- The aarch64 device-probe harness was rebuilt from the build checkout (see
  step (vii)).

**Why the build checkout is a separate worktree.** The release registration
pins `app_build` to `a07d8b7cea8f5bb86633447056df65080c4dd4db`, and the build
stamps HEAD into every export row. An APK built from any later commit,
including the commit that added this page, reads NOT MEASURED. So build and
run the probe guard from the detached checkout, and read this page and the
checklist from `mobile/triage-p6`. At a07d8b7 the scripts are not executable,
so call them through `bash`.

**Paste this block into every new terminal before any other command.** The
page spans several sessions, and without these variables every path below
expands to nothing.

```bash
W=/home/penguinzyue/cleophis-wt/p6-final-fix        # mobile/triage-p6: this page, the pipeline, the kit
B=/home/penguinzyue/cleophis-wt/mc1-build-a07d8b7   # detached at a07d8b7: the build and the probe guard
T=/home/penguinzyue/cleophas-triage-wt/p1i-TC2      # the triage checkout holding the release registration (the registration of record)
DESK="/mnt/c/Users/JM505 Computers/dev/cleophis"    # the desktop checkout: holds tools/pipeline/.env
PUBKEY=158cb99e9756e2e4d01d88b7ecfeb99a76547821ffe9f9f1985316c5451cd0c0   # kpack_core::sign::CURATOR_PUBLIC_KEY
DIST=https://cleophis-dist.s3.us-east-005.backblazeb2.com
APK=$HOME/cleophis-artifacts/cleophis-triage-r0-debug-a07d8b7.apk
J=$HOME/device-journey-r0                           # the hand-back folder
```

Every check below prints an OK line or a STOP line. **If you see STOP, stop
there** and send the output to the controller.

## (i) Sign the pack, then verify it (FOUNDER)

`sign_pack.py` reads the `.env` that sits next to it. The worktree has none,
so link the desktop checkout's. The link is gitignored, and step (iii) ends by
removing it. Your `.env` must already carry
`CURATOR_KEY_FILE=<absolute path to the curator key, outside the repo, mode 600>`.
The path must be absolute: a relative one is resolved against the directory
you run the command from, which differs between steps (i) and (iii).

```bash
ln -s "$DESK/tools/pipeline/.env" "$W/tools/pipeline/.env"
P=$B/src-tauri/resources/packs/reference-uk-v1.kpack
test "$(sha256sum "$P" | cut -c1-64)" = 5c7b2c98337118ecd8a6fbd07887a639be81371b4e325504997768b41cff1853 \
  && echo "OK: pack sha" || echo "STOP: pack sha MISMATCH, do not sign"
python3 "$W/tools/pipeline/sign_pack.py" --pack "$P"
python3 "$W/tools/pipeline/sign_pack.py" --pack "$P" --verify-with "$PUBKEY" \
  && echo "OK: signature verifies" || echo "STOP: signature does not verify"
cp "$P.sig" "$W/src-tauri/resources/packs/"
cp "$P.sig" "$DESK/src-tauri/resources/packs/"
```

You should also see `[sign] wrote …reference-uk-v1.kpack.sig (64 bytes)`.
The signed pair the build uses is the one in the BUILD checkout (`$B`); the
other two copies are a record. The desktop checkout's branch does not
gitignore `packs/`, so never `git add` there.

## (ii) Build the triage APK, then check it (agent or founder, no secret)

```bash
cd "$B"
test "$(git rev-parse HEAD)" = a07d8b7cea8f5bb86633447056df65080c4dd4db \
  && echo "OK: HEAD is a07d8b7" || echo "STOP: wrong HEAD, do not build"
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
print("OK: pack embedded" if pack in so else "STOP: the pack is NOT in the APK")
PY
```

`verify-apk.py` must end `VERDICT: PASS`. Send the four printed values to the
controller, who fills the build record in `phone-journey.md`.

**A known red that is not a build failure.** `check-mobile-build.sh` fails at
a07d8b7 before it compiles anything. Its acceptance-coverage step reports A1
(no CI workflow) and A5 (no `bmgr` backup test) as asserted but unbuilt. That
red predates wave C and says nothing about this APK, so do not stop for it.
MC1 ran the script's remaining steps on their own: the engine cross-compiles,
and the alignment probe library is 16 KB aligned (`0x4000`). The shipped
`libcleophis_lib.so` is linked 4 KB aligned (`0x1000`), which the probe does
not catch. That cannot affect a phone with 4 KB pages, such as the A22 on
Android 13. It is recorded for the controller as a spec H2 gap. To re-run
the script, expect exactly the A1 and A5 FAIL lines:

```bash
cd "$B"
ANDROID_NDK_ROOT=$HOME/android-ndk-r27c LIBCLANG_PATH=$HOME/.local/lib/python3.10/site-packages/clang/native \
  CARGO_TARGET_DIR=$HOME/cleophis-mobile-target bash docs/superpowers/mobile-tools/check-mobile-build.sh
git checkout -- docs/superpowers/mobile-tools/aarch64-align-probe/Cargo.lock   # the probe build rewrites it
```

## (iii) Sign and publish catalog v11, then verify it (FOUNDER)

**Everything in this step runs in ONE directory, `$W/tools/pipeline`.** Its
scripts write their outputs under their own directory (`work/catalog/`,
`work/out/`), and the relative paths you pass them resolve against the
directory you run from, so the two agree only there. This step inlines steps
1 to 3 of `publish-served-form.md`. Do not run that page's own `cd` lines:
they point at the old `cleophis-mobile` checkout.

```bash
cd "$W/tools/pipeline"
python3 -c "import boto3, cryptography" && echo "OK: python deps" || echo "STOP: install the deps (next line)"
# Only if STOP: python3 -m venv .venv && ./.venv/bin/pip install -r requirements.txt && . .venv/bin/activate
```

1. Fetch the two served files from `cleophis-models`. This reads the B2
   models pair through the triage repo's key loader, which reads the desktop
   `.env`. It prints only object names.

```bash
cd "$W/tools/pipeline"
mkdir -p work/out/Qwen3-1.7B/Q6_K work/out/Qwen3-1.7B/adapter
python3 - <<'PY'
import sys
sys.path.insert(0, "/home/penguinzyue/cleophas-triage")
from pipeline.env import require
import boto3

k = require("B2_ENDPOINT", "B2_MODELS_KEY_ID", "B2_MODELS_APP_KEY")
s3 = boto3.client("s3", endpoint_url=k["B2_ENDPOINT"],
                  aws_access_key_id=k["B2_MODELS_KEY_ID"], aws_secret_access_key=k["B2_MODELS_APP_KEY"])
prefix = "lineage/cleophas-triage/served/"
for name, dest in [("Qwen3-1.7B-Instruct-Q6_K.gguf", "work/out/Qwen3-1.7B/Q6_K/"),
                   ("triage-v3-Qwen3-1.7B.gguf", "work/out/Qwen3-1.7B/adapter/")]:
    for obj in (name, name + ".sha256"):
        s3.download_file("cleophis-models", prefix + obj, dest + obj)
        print("fetched", prefix + obj)
PY
```

2. Check the hashes. Five OK lines are required.

```bash
cd "$W/tools/pipeline"
BF=work/out/Qwen3-1.7B/Q6_K/Qwen3-1.7B-Instruct-Q6_K.gguf
AF=work/out/Qwen3-1.7B/adapter/triage-v3-Qwen3-1.7B.gguf
B_SHA=$(sha256sum "$BF" | cut -c1-64); A_SHA=$(sha256sum "$AF" | cut -c1-64)
test "$B_SHA" = 2588912fe87f55b8381b9fc8faacd0e905c9eb2db641a468be693f45ad6fc87a && echo "OK: base registered sha" || echo "STOP: base sha"
test "$B_SHA" = "$(cut -c1-64 "$BF.sha256")" && echo "OK: base pod sidecar" || echo "STOP: base sidecar"
test "$(stat -c %s "$BF")" = 1673006944 && echo "OK: base size" || echo "STOP: base size"
test "$A_SHA" = 5304e464cd485e8a7d8eb75083363e3cc4de0f665e2c785dbd1a1f7e93d13a20 && echo "OK: adapter registered sha" || echo "STOP: adapter sha"
test "$A_SHA" = "$(cut -c1-64 "$AF.sha256")" && echo "OK: adapter pod sidecar" || echo "STOP: adapter sidecar"
```

3. Write the two manifests. The adapter's size is measured from the file.

```bash
cd "$W/tools/pipeline"
cat > work/out/Qwen3-1.7B/Q6_K/manifest.json <<'EOF'
{
  "name": "Qwen3-1.7B-Instruct-Q6_K.gguf",
  "kind": "base",
  "base_model": "Qwen3-1.7B",
  "quant": "Q6_K",
  "sha256": "2588912fe87f55b8381b9fc8faacd0e905c9eb2db641a468be693f45ad6fc87a",
  "size": 1673006944,
  "license": "Apache-2.0",
  "source": "cleophis-models/lineage/cleophas-triage/served/Qwen3-1.7B-Instruct-Q6_K.gguf (built on the M8/M9/M10 pods, llama.cpp b10042)"
}
EOF
A_BYTES=$(stat -c %s work/out/Qwen3-1.7B/adapter/triage-v3-Qwen3-1.7B.gguf)
cat > work/out/Qwen3-1.7B/adapter/manifest.json <<EOF
{
  "name": "triage-v3-Qwen3-1.7B.gguf",
  "kind": "adapter",
  "base_model": "Qwen3-1.7B",
  "adapter_name": "triage",
  "version": "v3",
  "sha256": "5304e464cd485e8a7d8eb75083363e3cc4de0f665e2c785dbd1a1f7e93d13a20",
  "size": ${A_BYTES},
  "license": "Apache-2.0",
  "source": "cleophis-models/lineage/cleophas-triage/served/triage-v3-Qwen3-1.7B.gguf"
}
EOF
test -f work/out/Qwen3-1.7B/Q6_K/manifest.json && test -f work/out/Qwen3-1.7B/adapter/manifest.json \
  && echo "OK: both manifests under $PWD/work/out" || echo "STOP: a manifest is missing"
```

4. Build, check against the bundled pins, sign, publish and verify:

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
assert new["catalog_version"] == 11 and new["artifacts"][:11] == old, "STOP: carry-forward mismatch"
b, a = new["artifacts"][11:]
assert (b["kind"], b["sha256"], b["size"]) == ("base", e["sha256"], e["fileBytes"]), ("STOP", b)
assert (a["kind"], a["sha256"]) == ("adapter", e["adapterSha256"]), ("STOP", a)
assert a["size"] == os.path.getsize("work/out/Qwen3-1.7B/adapter/triage-v3-Qwen3-1.7B.gguf"), ("STOP", a)
assert os.path.basename(b["path"]) == os.path.basename(e["modelFile"]), "STOP: base basename"
assert os.path.basename(a["path"]) == os.path.basename(e["adapterFile"]), "STOP: adapter basename"
print("OK: v11 matches the bundled catalog.triage.json pins")
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
`catalog_version=11, 13 artifact(s)`. Do not sign if the pin check did not
print its OK line. `verify_published.py` must end with an `OK:` line.

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
bash "$W/docs/superpowers/mobile-tools/phone-run-kit.sh" prepare "$J"
```

The kit makes `exports/`, `exports-after-restart/`, `probe/` and
`screenshots/`, and a `founder-steps.json` listing all 21 steps with empty
results. It pushes `phone-journey-inputs.txt` to the phone's Downloads. Then,
on the phone's keyboard, turn off auto-capitalisation, autocorrect and smart
punctuation. The kit prints where those settings usually live; the menu
names were not checked on the A22. Paste every input.

## (vii) Run the checklist, and the device probes

Follow `$W/docs/superpowers/mobile-tools/phone-journey.md` from S1 to C1.
Record each step in `$J/founder-steps.json` as `"PASS"` or `"FAIL"`.
Anything else reads as NOT RECORDED. The airplane-mode harness for S4:

```bash
cd "$B" && bash docs/superpowers/mobile-tools/airplane-mode.sh --interactive 1800
```

Not over wireless debugging: the harness turns the radios off itself and
then verifies them over adb, so a wifi adb link dies under it, exactly as the
default probe run did. For S4, plug the phone in over USB and point the
harness at an adb that sees the USB device. On this laptop WSL has no USB
bus, but the Windows adb runs through interop, so prefix the command with
`ADB="/mnt/c/Users/JM505 Computers/AppData/Local/Android/Sdk/platform-tools/adb.exe"`
(the kit's `pull` and `pull-replay` take the same override, or stay on the
WSL adb over wireless once the radios are back). Confirm with
`"$ADB" devices` that the USB serial shows as `device` before starting the
window. After S4, airplane mode off, wireless debugging back on, and
`adb connect` again if the port changed.

Right after E1, before R1, pull the first exports into `exports/`. Doing
this later would also pull R1's re-export into `exports/`, and R1 would then
compare the post-restart log with itself:

```bash
bash "$W/docs/superpowers/mobile-tools/phone-run-kit.sh" pull "$J"   # E1: every *-triage-log.jsonl
```

For R1, pull the re-export by the T6 chat's title, as shown on the phone.
Leave the first copy on the phone: Android either replaces it or saves the
new one with " (1)" in its name, and the kit takes the newest match either
way. It saves it as `…-after-restart.jsonl`, so the first copy in `exports/`
is never replaced.

```bash
bash "$W/docs/superpowers/mobile-tools/phone-run-kit.sh" pull-replay "$J" '<the T6 chat title>'
```

**The device probes** need three things in place.

- **The models in one folder.** `run-device-probes.sh` looks for both GGUFs
  under one `--artifacts` folder, and step (iii) left them in two. Link them:

```bash
mkdir -p "$HOME/cleophis-artifacts"
ln -sf "$W/tools/pipeline/work/out/Qwen3-1.7B/Q6_K/Qwen3-1.7B-Instruct-Q6_K.gguf" "$HOME/cleophis-artifacts/"
ln -sf "$W/tools/pipeline/work/out/Qwen3-1.7B/adapter/triage-v3-Qwen3-1.7B.gguf" "$HOME/cleophis-artifacts/"
```

- **The aarch64 harness.** The script pushes
  `~/cleophis-mobile-target/aarch64-linux-android/release/examples/probe`.
  MC1 rebuilt it from `$B` on 2026-09-30, from source commit a07d8b7, with
  sha256 `4223857d10fe7358882071206f8ea23a1009278bf9a0b304ae8985dfc75ec564`.
  Its llama.cpp is built with the floor the a07d8b7 vendored
  `llama-cpp-sys-2` sets, `GGML_CPU_ARM_ARCH=armv8-a`, which means no forced
  `+dotprod`. The script's FATAL hint still
  says "with the dotprod patch applied", which is stale wording. The target
  folder is shared with other sessions, so check the harness first:

```bash
H=$HOME/cleophis-mobile-target/aarch64-linux-android/release/examples/probe
test "$(sha256sum "$H" | cut -c1-64)" = 4223857d10fe7358882071206f8ea23a1009278bf9a0b304ae8985dfc75ec564 \
  && echo "OK: harness is MC1's a07d8b7 build" || echo "STOP: harness changed; rebuild it (next block) and send the new sha"
```

```bash
cd "$B" && ANDROID_NDK_ROOT=$HOME/android-ndk-r27c ANDROID_NDK_HOME=$HOME/android-ndk-r27c NDK_ROOT=$HOME/android-ndk-r27c \
  ANDROID_NDK=$HOME/android-ndk-r27c LIBCLANG_PATH=$HOME/.local/lib/python3.10/site-packages/clang/native \
  CARGO_TARGET_DIR=$HOME/cleophis-mobile-target \
  cargo ndk -t arm64-v8a -P 24 build -p kpack-engine --release --features real --example probe
```

- **The prompt set, then the run, then the guard.** The gate and stack are the
  registration's: `work/gate-17b-m11/results-served` and
  `Qwen3-1.7B-armb-v3.pB`. That gives 260 prompts (crisis 40,
  crisis-embedded 20, endpoint 200).

```bash
cd "$T" && python3 -m pipeline.analysis.device_probes --gate-dir work/gate-17b-m11/results-served \
  --stack Qwen3-1.7B-armb-v3.pB --label release-r0 --out-dir "$J/probe"
cd "$B" && bash docs/superpowers/mobile-tools/run-device-probes.sh --no-airplane \
  --prompts "$J/probe/release-r0.jsonl" --artifacts "$HOME/cleophis-artifacts" --out "$J/probe"
cd "$B" && node probes/device-guard.mjs --in "$J/probe/release-r0.<serial>.json" \
  --out "$J/probe/release-r0.guard.jsonl"
```

`--no-airplane` is required when the phone is attached over wireless debugging:
without it the script enables airplane mode to prove offline inference, which
switches wifi off, drops its own adb link one second into the run, and leaves
the phone in airplane mode with wireless debugging off (seen on the first wave-C
run, 2026-10-01). The offline proof is step S4's `airplane-mode.sh` harness, a
separate registered step; the device bars read the probe replies and do not
depend on the radios. Over USB the flag may be omitted. If the link drops anyway,
re-run the same command: the pushed files and finished items persist on the
phone and the script skips the push.

## (viii) Export and hand back

The E1 exports were pulled in step (vii), right after E1. Delete any pulled
log in `exports/` that is not from this journey, then:

```bash
bash "$W/docs/superpowers/mobile-tools/phone-run-kit.sh" pack "$J"
```

Then tell the controller where `device-journey-r0.zip` is.

## What the controller does next

1. Fill the build record in `phone-journey.md` from the values sent in (ii)
   and (iii), and commit it on `mobile/triage-p6`.
2. Commit the post-publish fill amendment to the release registration,
   alone (done 2026-10-01: amendment 3, `artifacts/mvp-release-gate-prereg-a3.json`).
   It fills the catalog dist v11 sha, which the earlier amendments left `null`
   as "FILLED BY AMENDMENT AFTER PUBLISH", and renames every registered
   command to the new file. It does not record the APK sha: that lives in the
   build record of `phone-journey.md` and in the APK's provenance file.
3. Run the reader, the last block of `phone-journey.md`, from the triage
   checkout. It uses the reader's default (the registration of record), whose
   `device_journey.reading` it follows. Pass `--registration` only if the
   controller tells you to:

```bash
cd "$T" && python3 -m pipeline.analysis.device_journey \
  --exports "$J/exports" --replay-exports "$J/exports-after-restart" \
  --founder-steps "$J/founder-steps.json" \
  --prompt-set "$J/probe/release-r0.jsonl" --device-json "$J/probe/release-r0.<serial>.json" \
  --device-guard-json "$J/probe/release-r0.guard.jsonl" \
  --served-dir work/gate-17b-m11/results-served --stack Qwen3-1.7B-armb-v3.pB \
  --out artifacts/device-journey-r0.json
```

4. Then run the release gate, Task TC2b, with
   `--device-delta-json artifacts/device-journey-r0.json --gate-kind release`.
   The phone ships the shipped adapter v3 only if the gate passes.
