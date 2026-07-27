#!/usr/bin/env python3
"""Per-second busyness map for candidate overlay zones.

Overlay panels must not land on a video's own B-roll. Rather than guessing, score
every second of the clip for visual activity inside each candidate zone: a plain
wall scores near zero, a pasted-in article screenshot scores high. A generator can
then place panels "randomly" among only the zones that are actually free.

Usage:
  zone-analyze.py <thumb-dir> <out.json> [--width 1920] [--height 1080]
                  [--zones zones.json] [--fps 1]

<thumb-dir> holds frames extracted at --fps (default 1/sec), any scale; the script
infers the scale factor from the first image against --width. zones.json maps a
zone name to [x0,y0,x1,y1] in FULL-RESOLUTION pixels; omit it for the default
left/right/bottom thirds.
"""
import json
import os
import sys

import numpy as np
from PIL import Image

DEFAULT_ZONES_1080 = {
    "left": [70, 330, 660, 800],
    "right": [1260, 330, 1850, 800],
    "bottom": [500, 790, 1420, 1020],
}


def busyness(arr):
    """Edge energy + luminance spread. Flat backdrop -> near 0; graphics -> high."""
    if arr.size == 0:
        return 999.0
    gy, gx = np.gradient(arr.astype(np.float32))
    edges = float(np.sqrt(gx * gx + gy * gy).mean())
    return edges * 2.0 + float(arr.std()) * 0.25


def main(argv):
    if len(argv) < 3:
        print(__doc__)
        return 2
    thumb_dir, out_path = argv[1], argv[2]
    width, height, fps, zones_file = 1920, 1080, 1.0, None
    i = 3
    while i < len(argv):
        if argv[i] == "--width":
            width = int(argv[i + 1]); i += 2
        elif argv[i] == "--height":
            height = int(argv[i + 1]); i += 2
        elif argv[i] == "--fps":
            fps = float(argv[i + 1]); i += 2
        elif argv[i] == "--zones":
            zones_file = argv[i + 1]; i += 2
        else:
            raise SystemExit("unknown flag: " + argv[i])

    zones = json.load(open(zones_file)) if zones_file else dict(DEFAULT_ZONES_1080)
    if not zones_file and (width, height) != (1920, 1080):
        # scale the default thirds to the real canvas
        sx, sy = width / 1920.0, height / 1080.0
        zones = {k: [int(v[0] * sx), int(v[1] * sy), int(v[2] * sx), int(v[3] * sy)]
                 for k, v in zones.items()}

    files = sorted(f for f in os.listdir(thumb_dir)
                   if f.lower().endswith((".jpg", ".jpeg", ".png")))
    if not files:
        raise SystemExit("no thumbnails found in " + thumb_dir)

    first = Image.open(os.path.join(thumb_dir, files[0]))
    scale = width / float(first.width)  # full-res px per thumbnail px

    out = {}
    for idx, fn in enumerate(files):
        a = np.asarray(Image.open(os.path.join(thumb_dir, fn)).convert("L"))
        sec = int(round(idx / fps))
        row = {}
        for name, (x0, y0, x1, y1) in zones.items():
            patch = a[int(y0 / scale):int(y1 / scale), int(x0 / scale):int(x1 / scale)]
            row[name] = round(busyness(patch), 2)
        out[sec] = row

    with open(out_path, "w") as fh:
        json.dump(out, fh)

    for name in zones:
        vals = [out[s][name] for s in out]
        vals_sorted = sorted(vals)
        med = vals_sorted[len(vals_sorted) // 2]
        p90 = vals_sorted[int(len(vals_sorted) * 0.9)]
        print("%-8s median=%7.2f p90=%7.2f max=%7.2f" % (name, med, p90, max(vals)))
    print("seconds analyzed: %d -> %s" % (len(out), out_path))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
