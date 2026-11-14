#!/usr/bin/env python3
"""Generates the vision probe set v1: 16 typeset maths problems, two photographs of the printed sheet
(q19, q20: registered and hashed, never rendered), and four typeset refusal probes (two diagrams, two prose).

  python3 gen.py          write the 20 PNGs, items.json, manifest.json next to this file (22 items, photos hashed)
  python3 gen.py --check  re-render the 20 PNGs to a temp dir and compare sha256s with the committed files, and
                          check the photos' hashes against the manifest (exit 1 on any difference)
  python3 gen.py --out DIR [--fontset NAME]
                          scratch render: the 20 PNGs + items.json into DIR (no manifest); DIR may not be inside the
                          fixture directory; NAME is any matplotlib mathtext.fontset (default cm)
  python3 gen.py --self-test  scratch-render dejavusans to a temp dir, and check the default render still matches the committed set

Rendered with matplotlib mathtext in the Computer Modern fontset (the transcriber was trained on
LaTeX-rendered formulas), 512x160 px, white, each formula scaled to fill ~78% of the width.
"""
import hashlib
import json
import re
import sys
import tempfile
from pathlib import Path

import matplotlib

matplotlib.use("Agg")
matplotlib.rcParams["mathtext.fontset"] = "cm"
import matplotlib.pyplot as plt  # noqa: E402

HERE = Path(__file__).resolve().parent
FS = 36          # starting size; each item is rescaled to fill the crop
FS_MIN, FS_MAX = 28.0, 72.0
FILL_W = 0.78    # target share of the 512 px width
FILL_H = 0.80    # all lines together must stay within this share of the 160 px height

# (id-slug, kind, mathtext lines, expected latex, plain reading, notes)
PRINTED = [
    ("q01-linear", [r"$3x - 7 = 2x + 5$"], r"3x - 7 = 2x + 5", "three x minus seven equals two x plus five", "plain linear equation"),
    ("q02-fraction", [r"$\frac{2x}{3} + \frac{x}{6} = 5$"], r"\frac{2x}{3} + \frac{x}{6} = 5", "two x over three plus x over six equals five", "stacked fractions"),
    ("q03-quadratic", [r"$x^2 - 5x + 6 = 0$"], r"x^2 - 5x + 6 = 0", "x squared minus five x plus six equals zero", "superscript"),
    ("q04-exponent-times", [r"$2^{n+1} \times a_n \le 10$"], r"2^{n+1} \times a_n \le 10", "two to the n plus one times a sub n is at most ten", "braced exponent, subscript, times, le"),
    ("q05-system", [r"$x + y = 7$", r"$x - y = 1$"], r"x + y = 7 \\ x - y = 1", "x plus y equals seven, and x minus y equals one", "two stacked lines; expected latex joins with a double backslash"),
    ("q06-sqrt", [r"$\sqrt{x + 1} = 3$"], r"\sqrt{x + 1} = 3", "square root of x plus one equals three", "radical"),
    ("q07-negative-coeff", [r"$-4x + 9 = -7$"], r"-4x + 9 = -7", "negative four x plus nine equals negative seven", "leading minus"),
    ("q08-mixed-fraction", [r"$\frac{3}{4}x - \frac{1}{2} = \frac{5}{8}$"], r"\frac{3}{4}x - \frac{1}{2} = \frac{5}{8}", "three quarters x minus one half equals five eighths", "three fractions"),
    ("q09-subscripts", [r"$a_1 + a_2 = 2a_3$"], r"a_1 + a_2 = 2a_3", "a sub one plus a sub two equals two a sub three", "subscripts"),
    ("q10-decimal", [r"$0.5x + 1.25 = 3$"], r"0.5x + 1.25 = 3", "zero point five x plus one point two five equals three", "decimals"),
    ("q11-parentheses", [r"$2(x - 3) = 4(x + 1)$"], r"2(x - 3) = 4(x + 1)", "two times x minus three equals four times x plus one", "parentheses"),
    ("q12-inequality", [r"$3x - 2 \ge 7$"], r"3x - 2 \ge 7", "three x minus two is at least seven", "ge"),
    ("q13-power-of-fraction", [r"$\left(\frac{1}{2}\right)^x = 8$"], r"\left(\frac{1}{2}\right)^x = 8", "one half to the x equals eight", "sized parentheses, power of a fraction"),
    ("q14-percent", [r"$15\% \times 80 = x$"], r"15\% \times 80 = x", "fifteen percent times eighty equals x", "percent sign"),
    ("q15-text-and-formula", [r"Solve for x: $5x = 35$"], r"5x = 35", "solve for x: five x equals thirty-five", "prose outside the maths; expected latex is the formula only"),
    ("q16-cubic", [r"$x^3 - x = 0$"], r"x^3 - x = 0", "x cubed minus x equals zero", "cubic"),
]


# The founder's photographs of the printed sheet (CC0). Crops, one per orientation; EXIF holds only the
# Orientation tag. Registered here, hashed into manifest.json, never rendered or rewritten.
# (id, file, latex, plain, notes)
PHOTOS = [
    ("q19", "photos/q19-photo-upright.jpg", r"x^2 - 5x + 6 = 0", "x squared minus five x plus six equals zero",
     "crop of the founder's photograph of the printed sheet (q03's formula); CC0; EXIF orientation 1"),
    ("q20", "photos/q20-photo-rotated.jpg", r"x^2 - 5x + 6 = 0", "x squared minus five x plus six equals zero",
     "crop of the founder's photograph of the printed sheet (q03's formula); CC0; raw pixels sideways, EXIF orientation 6, "
     "upright only once the orientation is applied"),
]


def new_fig():
    return plt.figure(figsize=(5.12, 1.6), dpi=100, facecolor="white")


def save(fig, path):
    fig.savefig(path, dpi=100, facecolor="white", metadata={"Software": None})
    plt.close(fig)


def mathtext(line):
    # mathtext knows \leq/\geq but not \le/\ge; the canonical LaTeX in items.json keeps the short forms.
    return re.sub(r"\\(le|ge)(?![a-z])", r"\\\1q", line)


def _extent(fig, lines, fs, ys):
    """Width and total height (px) of the rendered lines at size fs."""
    r = fig.canvas.get_renderer()
    texts = [fig.text(0.5, y, ln, fontsize=fs, ha="center", va="center") for y, ln in zip(ys, lines)]
    boxes = [t.get_window_extent(r) for t in texts]
    for t in texts:
        t.remove()
    return max(bx.width for bx in boxes), max(bx.y1 for bx in boxes) - min(bx.y0 for bx in boxes)


def render_printed(path, lines):
    lines = [mathtext(ln) for ln in lines]
    fig = new_fig()
    ys = (0.5,) if len(lines) == 1 else (0.70, 0.30)
    w, h = _extent(fig, lines, FS, ys)
    scale = min(FILL_W * 512 / w, FILL_H * 160 / h)
    fs = round(max(FS_MIN, min(FS_MAX, FS * scale)), 1)
    w, h = _extent(fig, lines, fs, ys)
    assert w <= 512 * 0.95 and h <= 160 * 0.95, (lines, fs, w, h)  # nothing clipped
    for y, ln in zip(ys, lines):
        fig.text(0.5, y, ln, fontsize=fs, ha="center", va="center")
    save(fig, path)


def render_diagram(path):
    fig = new_fig()
    ax = fig.add_axes([0.3, 0.1, 0.4, 0.8])
    A, B, C = (0, 0), (1.6, 0), (0, 1)
    xs, ys = zip(A, B, C, A)
    ax.plot(xs, ys, color="black", linewidth=2)
    ax.plot([0, 0.15, 0.15], [0.15, 0.15, 0], color="black", linewidth=1.5)
    for label, (x, y), dx, dy in (("A", A, -0.18, -0.12), ("B", B, 0.1, -0.12), ("C", C, -0.18, 0.04)):
        ax.text(x + dx, y + dy, label, fontsize=22, ha="center", va="center")
    ax.set_xlim(-0.4, 1.9)
    ax.set_ylim(-0.3, 1.2)
    ax.axis("off")
    save(fig, path)


def render_prose(path):
    fig = new_fig()
    for y, ln in zip((0.78, 0.5, 0.22), ("Shopping list for Saturday:", "milk, eggs, bread, butter,", "apples and a jar of honey.")):
        fig.text(0.5, y, ln, fontsize=24, ha="center", va="center")
    save(fig, path)


def render_diagram_345(path):
    fig = new_fig()
    ax = fig.add_axes([0.3, 0.1, 0.4, 0.8])
    A, B, C = (0, 0), (4, 0), (0, 3)
    xs, ys = zip(A, B, C, A)
    ax.plot(xs, ys, color="black", linewidth=2)
    ax.plot([0, 0.3, 0.3], [0.3, 0.3, 0], color="black", linewidth=1.5)
    ax.text(2, -0.45, "4", fontsize=22, ha="center", va="center")
    ax.text(-0.4, 1.5, "3", fontsize=22, ha="center", va="center")
    ax.text(2.3, 1.8, "5", fontsize=22, ha="center", va="center")
    ax.set_xlim(-1.0, 4.6)
    ax.set_ylim(-1.0, 3.5)
    ax.axis("off")
    save(fig, path)


def render_dated_prose(path):
    fig = new_fig()
    fig.text(0.5, 0.5, "Dentist on 14 November 2026 at 3 pm", fontsize=17, ha="center", va="center")
    save(fig, path)


def build(outdir: Path):
    items = []
    for slug, lines, latex, plain, notes in PRINTED:
        f = slug + ".png"
        render_printed(outdir / f, lines)
        items.append({"id": slug[:3], "file": f, "kind": "printed", "latex": latex, "plain": plain, "verdict": "accept", "notes": notes})
    render_diagram(outdir / "q17-diagram-only.png")
    items.append({"id": "q17", "file": "q17-diagram-only.png", "kind": "diagram", "latex": None,
                  "plain": "a labelled right triangle with vertices A, B and C; no equation", "verdict": "refuse",
                  "notes": "diagram only; nothing to transcribe"})
    render_prose(outdir / "q18-prose.png")
    items.append({"id": "q18", "file": "q18-prose.png", "kind": "prose", "latex": None,
                  "plain": "three lines of plain English: a shopping list", "verdict": "refuse",
                  "notes": "prose only; no mathematics"})
    for id_, file, latex, plain, notes in PHOTOS:
        items.append({"id": id_, "file": file, "kind": "photo", "latex": latex, "plain": plain, "verdict": "accept", "notes": notes})
    render_diagram_345(outdir / "q21-diagram-345.png")
    items.append({"id": "q21", "file": "q21-diagram-345.png", "kind": "diagram", "latex": None,
                  "plain": "a right triangle drawn with lines, its sides labelled 3, 4 and 5; no equation", "verdict": "refuse",
                  "notes": "held-out refusal probe: a diagram with numbers, nothing to transcribe"})
    render_dated_prose(outdir / "q22-dated-prose.png")
    items.append({"id": "q22", "file": "q22-dated-prose.png", "kind": "prose", "latex": None,
                  "plain": "one line of plain English holding a date and a time", "verdict": "refuse",
                  "notes": "held-out refusal probe: prose with digits, no mathematics"})
    items.sort(key=lambda it: it["id"])
    return items


def rendered(items):
    return [it for it in items if it["kind"] != "photo"]


def sha(p: Path):
    return hashlib.sha256(p.read_bytes()).hexdigest()


def dump(obj):
    return json.dumps(obj, indent=2, ensure_ascii=False) + "\n"


def arg(name):
    if name not in sys.argv:
        return None
    i = sys.argv.index(name)
    if i + 1 >= len(sys.argv):
        sys.exit(f"{name} needs a value")
    return sys.argv[i + 1]


def check():
    with tempfile.TemporaryDirectory() as td:
        items = build(Path(td))
        bad = []
        manifest = json.loads((HERE / "manifest.json").read_text())
        for it in rendered(items):
            got = sha(Path(td) / it["file"])
            committed = HERE / it["file"]
            if not committed.exists() or sha(committed) != got or manifest["files"].get(it["file"]) != got:
                bad.append(it["file"])
        for it in items:
            if it["kind"] == "photo":
                committed = HERE / it["file"]
                if not committed.exists() or manifest["files"].get(it["file"]) != sha(committed):
                    bad.append(it["file"])
        if manifest.get("count") != len(items):
            bad.append("manifest count")
        if (HERE / "items.json").read_text() != dump({"version": 1, "items": items}):
            bad.append("items.json")
    return bad


def self_test():
    with tempfile.TemporaryDirectory() as td:
        matplotlib.rcParams["mathtext.fontset"] = "dejavusans"
        items = build(Path(td))
        (Path(td) / "items.json").write_text(dump({"version": 1, "items": items}))
        assert len(list(Path(td).glob("*.png"))) == 20 and (Path(td) / "items.json").exists(), "scratch render incomplete"
    matplotlib.rcParams["mathtext.fontset"] = "cm"
    bad = check()
    assert not bad, f"default render no longer matches the committed set: {bad}"
    print("ok: dejavusans scratch render has 20 PNGs + items.json; default render matches the committed hashes")


def main():
    if "--self-test" in sys.argv:
        self_test()
        return
    out, fontset = arg("--out"), arg("--fontset")
    if "--check" in sys.argv:
        if out or fontset:
            sys.exit("--check verifies the committed set; it takes no --out or --fontset")
        bad = check()
        if bad:
            print("DIFFERENT:", ", ".join(bad))
            sys.exit(1)
        print("ok: 20 PNGs and items.json reproduce byte-for-byte, 2 photos match the manifest")
        return
    if fontset and not out:
        sys.exit("--fontset only makes sense with --out; the committed set is Computer Modern")
    if fontset:
        matplotlib.rcParams["mathtext.fontset"] = fontset
    if out:
        dest = Path(out).resolve()
        if dest == HERE or HERE in dest.parents:
            sys.exit(f"refusing to render into the fixture directory: {dest}")
        dest.mkdir(parents=True, exist_ok=True)
        items = build(dest)
        (dest / "items.json").write_text(dump({"version": 1, "items": items}))
        print(f"wrote {len(rendered(items))} PNGs and items.json ({len(items)} items) to {dest} (fontset {fontset or 'cm'}; no manifest)")
        return
    items = build(HERE)
    (HERE / "items.json").write_text(dump({"version": 1, "items": items}))
    for it in items:
        if it["kind"] == "photo" and not (HERE / it["file"]).exists():
            sys.exit(f"missing photograph {it['file']}: it is never rendered")
    files = {it["file"]: sha(HERE / it["file"]) for it in items}
    (HERE / "manifest.json").write_text(dump({"version": 1, "count": len(files), "files": files}))
    print(f"wrote {len(files)} files (20 PNGs, 2 photos), items.json, manifest.json")


if __name__ == "__main__":
    main()
