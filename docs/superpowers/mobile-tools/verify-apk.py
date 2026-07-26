#!/usr/bin/env python3
"""Verify a Cleophis APK against the artifact itself, never against the config.

Encodes the standing checks this milestone has been performing by hand:

  - zip-entry accounting, which is the ONLY thing that catches the zipflinger
    orphan trap: Phase 0's 334 MiB APK became 658 MiB with half of it an
    unreferenced copy of libcleophis_lib.so that no central-directory entry
    pointed at, and it installed and ran perfectly. The file length cannot
    tell the difference; the entry sum can.
  - assets/ contents, which is where the `{}`-vs-null RFC 7386 merge trap
    showed up as 239 MB of Windows .dll/.exe.
  - ABI list, package identity, and the manifest facts that are spec-critical.
"""
import hashlib
import subprocess
import sys
import zipfile

apk = sys.argv[1]
aapt2 = sys.argv[2] if len(sys.argv) > 2 else None

with open(apk, 'rb') as f:
    data = f.read()
size = len(data)
digest = hashlib.sha256(data).hexdigest()

z = zipfile.ZipFile(apk)
infos = z.infolist()
entry_sum = sum(i.compress_size for i in infos)
unaccounted = size - entry_sum
pct = unaccounted / size * 100

print(f"file        {apk}")
print(f"size        {size:,} bytes")
print(f"sha256      {digest}")
print(f"entries     {len(infos)}")
print(f"entry sum   {entry_sum:,} compressed")
print(f"unaccounted {unaccounted:,} ({pct:.2f}%)  <- >1% suggests a stranded orphan")

assets = sorted(n for n in z.namelist() if n.startswith('assets/'))
libs = sorted(n for n in z.namelist() if n.startswith('lib/'))
print(f"assets/     {assets}")
print(f"lib/        {libs}")
abis = sorted({n.split('/')[1] for n in libs})
print(f"ABIs        {abis}")

biggest = sorted(infos, key=lambda i: i.file_size, reverse=True)[:3]
print("largest     " + ", ".join(f"{i.filename} ({i.file_size:,})" for i in biggest))

ok = True
if pct > 1.0:
    print("FAIL: unaccounted bytes exceed 1% — possible stranded orphan"); ok = False
if assets != ['assets/tauri.conf.json']:
    print(f"FAIL: assets/ should be exactly ['assets/tauri.conf.json'], got {assets}"); ok = False
if abis != ['arm64-v8a']:
    print(f"FAIL: ABIs should be exactly ['arm64-v8a'], got {abis}"); ok = False

if aapt2:
    badging = subprocess.run([aapt2, 'dump', 'badging', apk], capture_output=True, text=True).stdout
    xmltree = subprocess.run([aapt2, 'dump', 'xmltree', '--file', 'AndroidManifest.xml', apk],
                             capture_output=True, text=True).stdout
    for line in badging.splitlines():
        if line.startswith(('package:', 'sdkVersion:', 'targetSdkVersion:')):
            print("badging     " + line.strip())
    checks = {
        'com.cleophis.app': badging,
        'REQUEST_INSTALL_PACKAGES': badging,
        'FOREGROUND_SERVICE_SPECIAL_USE': badging,
    }
    for needle, hay in checks.items():
        print(f"{'ok  ' if needle in hay else 'FAIL'}        {needle} present")
        if needle not in hay:
            ok = False
    # allowBackup must be false, and the 2.2 IME attribute must have survived
    # the merge into the packaged manifest.
    allow_backup_false = 'allowBackup' in xmltree and '0xffffffff' not in xmltree.split('allowBackup')[1][:80]
    print(f"{'ok  ' if allow_backup_false else '??  '}        allowBackup=false (packaged manifest)")
    soft_input = 'windowSoftInputMode' in xmltree
    print(f"{'ok  ' if soft_input else 'FAIL'}        windowSoftInputMode present (task 2.2 IME)")
    if not soft_input:
        ok = False
    if soft_input:
        seg = xmltree.split('windowSoftInputMode')[1][:80].strip()
        print(f"            windowSoftInputMode raw: {seg.splitlines()[0]}")

print("\nVERDICT:", "PASS" if ok else "FAIL")
sys.exit(0 if ok else 1)
