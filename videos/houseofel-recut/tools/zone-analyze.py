"""Build a per-second busyness map for the three candidate overlay zones.

Thumbnails are 384x216 (1920x1080 / 5). A zone is "free" when it is visually
quiet — a plain wall scores near zero, an article screenshot or B-roll scores
high. Busyness = mean gradient magnitude (edges) + luminance spread, which
separates flat backdrop from any pasted-in graphic very cleanly.
"""

import json
import os

import numpy as np
from PIL import Image

SRC = "/home/penguinzyue/.claude/jobs/a5b95366/tmp/zonemap"
S = 5  # thumbnail scale factor vs 1920x1080

# zone -> full-res box (x0, y0, x1, y1)
ZONES = {
    "left": (60, 300, 660, 800),
    "right": (1260, 300, 1860, 800),
    "bottom": (460, 790, 1460, 1020),
}


def busyness(arr):
    """Edge energy + luminance spread of a grayscale patch."""
    if arr.size == 0:
        return 999.0
    gy, gx = np.gradient(arr.astype(np.float32))
    edges = float(np.sqrt(gx * gx + gy * gy).mean())
    spread = float(arr.std())
    return edges * 2.0 + spread * 0.25


files = sorted(f for f in os.listdir(SRC) if f.endswith(".jpg"))
out = {}
for i, fn in enumerate(files):
    im = Image.open(os.path.join(SRC, fn)).convert("L")
    a = np.asarray(im)
    sec = i  # fps=1 → frame i is second i
    row = {}
    for name, (x0, y0, x1, y1) in ZONES.items():
        patch = a[y0 // S : y1 // S, x0 // S : x1 // S]
        row[name] = round(busyness(patch), 2)
    out[sec] = row

with open("/home/penguinzyue/.claude/jobs/a5b95366/tmp/zonemap.json", "w") as fh:
    json.dump(out, fh)

# summary
import statistics as st

for name in ZONES:
    vals = [out[s][name] for s in out]
    print(
        f"{name:7s} median={st.median(vals):7.2f}  p10={np.percentile(vals,10):7.2f}  "
        f"p90={np.percentile(vals,90):7.2f}  max={max(vals):7.2f}"
    )
print(f"\nseconds analyzed: {len(out)}")
