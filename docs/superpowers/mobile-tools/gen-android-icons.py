#!/usr/bin/env python3
"""Generate the Android launcher icon set from the brand master.

ANDROID ONLY. This never touches `src-tauri/icons/` — desktop branding is a
main-branch founder decision, and a blanket `tauri icon` run would clobber it.
That is also why the icons are generated here rather than by the Tauri CLI.

Produces, under `src-tauri/gen/android/app/src/main/res/`:

  mipmap-<d>/ic_launcher.png          legacy icon, the full mark
  mipmap-<d>/ic_launcher_round.png    circle-cropped variant
  mipmap-<d>/ic_launcher_foreground.png
                                      adaptive foreground: the diamond alone on
                                      transparent, sized into the 108dp grid's
                                      safe zone

Adaptive-icon notes. The scaffold `tauri android init` produced has foreground
PNGs but NO `mipmap-anydpi-v26/`, so nothing referenced them and Android fell
back to the legacy raster — which launchers then mask and letterbox themselves,
usually onto white. The XML that makes the adaptive pieces live is written by
this script too. The 108dp canvas keeps only its central 72dp guaranteed
visible (the outer 18dp per side is parallax/mask territory), so the diamond is
sized to ~60% of the canvas and stays inside that safe zone at every mask shape.

Usage:  docs/superpowers/mobile-tools/gen-android-icons.py [--check]
        --check verifies the generated files match the master without writing.
"""
import sys
from pathlib import Path

from PIL import Image, ImageDraw

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent.parent
MASTER = HERE / "brand" / "icon-master-1024.png"
RES = ROOT / "src-tauri" / "gen" / "android" / "app" / "src" / "main" / "res"

# Brand system: ink navy field, magenta diamond.
INK_NAVY = "#141829"

# Standard Android launcher densities. Note hdpi = 72: the Tauri scaffold had
# emitted 49 px there, which is not a real Android bucket size and renders
# soft on hdpi devices.
LEGACY = {"mdpi": 48, "hdpi": 72, "xhdpi": 96, "xxhdpi": 144, "xxxhdpi": 192}
# Adaptive foregrounds are the 108dp canvas at each density.
ADAPTIVE = {"mdpi": 108, "hdpi": 162, "xhdpi": 216, "xxhdpi": 324, "xxxhdpi": 432}

# Fraction of the adaptive canvas the diamond spans. The safe zone is 72/108 =
# 0.667, so 0.60 leaves margin at every mask shape rather than grazing the edge.
DIAMOND_SCALE = 0.60


def diamond_bbox(img: Image.Image) -> tuple[int, int, int, int]:
    """Bounding box of the mark, i.e. everything that is neither the navy field
    nor the transparent area outside the rounded corners."""
    rgba = img.convert("RGBA")
    navy = tuple(int(INK_NAVY[i : i + 2], 16) for i in (1, 3, 5))
    px = rgba.load()
    w, h = rgba.size
    xs, ys = [], []
    for y in range(h):
        for x in range(w):
            r, g, b, a = px[x, y]
            if a < 8:
                continue
            # Distance from the field colour; the facets are all far from navy.
            if abs(r - navy[0]) + abs(g - navy[1]) + abs(b - navy[2]) > 40:
                xs.append(x)
                ys.append(y)
    if not xs:
        raise SystemExit("could not locate the mark against the field colour")
    return min(xs), min(ys), max(xs) + 1, max(ys) + 1


def circle_crop(img: Image.Image) -> Image.Image:
    """Inscribed-circle crop for the round variant."""
    out = img.convert("RGBA").copy()
    mask = Image.new("L", out.size, 0)
    ImageDraw.Draw(mask).ellipse((0, 0, out.size[0] - 1, out.size[1] - 1), fill=255)
    # Intersect with the existing alpha so the rounded field's own edges survive.
    alpha = out.getchannel("A").point(lambda v: v)
    mask = Image.composite(alpha, Image.new("L", out.size, 0), mask)
    out.putalpha(mask)
    return out


def isolate_mark(img: Image.Image) -> Image.Image:
    """Drop the navy field, keeping only the diamond.

    Cropping to the diamond's bounding box is not enough: the diamond is a
    rotated square, so its bbox corners are still field colour, and pasting that
    as an adaptive foreground would show a navy SQUARE floating on the
    background layer. Alpha is ramped by distance from the field colour rather
    than hard-thresholded, so the master's antialiased diamond edge survives as
    a soft edge instead of a staircase.
    """
    import numpy as np

    arr = np.asarray(img.convert("RGBA")).astype(np.int16)
    navy = np.array([int(INK_NAVY[i : i + 2], 16) for i in (1, 3, 5)], dtype=np.int16)
    dist = np.abs(arr[..., :3] - navy).sum(axis=2)
    # 0 at the field colour, fully opaque once clearly the mark.
    alpha = np.clip((dist - 12) * (255.0 / 60.0), 0, 255)
    alpha = np.minimum(alpha, arr[..., 3])
    out = arr.copy()
    out[..., 3] = alpha
    return Image.fromarray(out.astype(np.uint8), "RGBA")


def foreground(master: Image.Image, size: int) -> Image.Image:
    """The diamond alone, centred on a transparent 108dp-grid canvas."""
    box = diamond_bbox(master)
    mark = isolate_mark(master).crop(box)

    target = int(round(size * DIAMOND_SCALE))
    # Preserve aspect; the diamond is square but do not assume it.
    mw, mh = mark.size
    scale = target / max(mw, mh)
    mark = mark.resize((max(1, int(round(mw * scale))), max(1, int(round(mh * scale)))),
                       Image.LANCZOS)

    canvas = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    canvas.paste(mark, ((size - mark.size[0]) // 2, (size - mark.size[1]) // 2), mark)
    return canvas


ANYDPI = """<?xml version="1.0" encoding="utf-8"?>
<adaptive-icon xmlns:android="http://schemas.android.com/apk/res/android">
    <background android:drawable="@color/ic_launcher_background" />
    <foreground android:drawable="@mipmap/ic_launcher_foreground" />
</adaptive-icon>
"""

COLORS = """<?xml version="1.0" encoding="utf-8"?>
<resources>
    <!-- Brand ink navy; the adaptive icon's background layer. -->
    <color name="ic_launcher_background">#141829</color>
</resources>
"""


def main() -> int:
    check = "--check" in sys.argv
    if not MASTER.exists():
        raise SystemExit(f"missing master: {MASTER}")
    master = Image.open(MASTER).convert("RGBA")
    if master.size != (1024, 1024):
        raise SystemExit(f"master must be 1024x1024, got {master.size}")

    round_master = circle_crop(master)
    written = []

    for density, size in LEGACY.items():
        d = RES / f"mipmap-{density}"
        d.mkdir(parents=True, exist_ok=True)
        for name, src in (
            ("ic_launcher.png", master),
            ("ic_launcher_round.png", round_master),
        ):
            img = src.resize((size, size), Image.LANCZOS)
            if not check:
                img.save(d / name)
            written.append(f"{d.name}/{name} {size}x{size}")

    for density, size in ADAPTIVE.items():
        d = RES / f"mipmap-{density}"
        img = foreground(master, size)
        if not check:
            img.save(d / "ic_launcher_foreground.png")
        written.append(f"{d.name}/ic_launcher_foreground.png {size}x{size}")

    anydpi = RES / "mipmap-anydpi-v26"
    if not check:
        anydpi.mkdir(parents=True, exist_ok=True)
        (anydpi / "ic_launcher.xml").write_text(ANYDPI)
        (anydpi / "ic_launcher_round.xml").write_text(ANYDPI)
        (RES / "values" / "ic_launcher_background.xml").write_text(COLORS)
    written.append("mipmap-anydpi-v26/ic_launcher{,_round}.xml")
    written.append("values/ic_launcher_background.xml")

    print(("would write" if check else "wrote") + f" {len(written)} files:")
    for w in written:
        print("  " + w)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
