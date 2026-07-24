#!/usr/bin/env python3
"""Zip extractor that preserves unix symlinks + permission bits.

python's `zipfile -e` drops both: symlinks come out as text files holding the
target path, and exec bits vanish. The NDK is full of symlinks (clang ->
clang-18) so the stock extraction corrupts the toolchain. This restores both.
"""
import os, stat, sys, zipfile, shutil

src, dest = sys.argv[1], sys.argv[2]
zf = zipfile.ZipFile(src)
n_links = n_files = n_dirs = 0
for info in zf.infolist():
    mode = info.external_attr >> 16
    target = os.path.join(dest, info.filename)
    if stat.S_ISLNK(mode):
        linkto = zf.read(info).decode()
        os.makedirs(os.path.dirname(target), exist_ok=True)
        if os.path.lexists(target):
            os.remove(target)
        os.symlink(linkto, target)
        n_links += 1
    elif info.is_dir():
        os.makedirs(target, exist_ok=True)
        n_dirs += 1
    else:
        os.makedirs(os.path.dirname(target), exist_ok=True)
        # A regular file may have replaced a symlink on a prior bad extract.
        if os.path.islink(target):
            os.remove(target)
        with zf.open(info) as s, open(target, "wb") as d:
            shutil.copyfileobj(s, d)
        if mode & 0o777:
            os.chmod(target, mode & 0o777)
        n_files += 1
print(f"files={n_files} symlinks={n_links} dirs={n_dirs}")
