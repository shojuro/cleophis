#!/usr/bin/env python3
"""Generates the vision probe set v1: 16 typeset maths problems, one diagram, one page of prose.

  python3 gen.py          write the 18 PNGs, items.json, manifest.json next to this file
  python3 gen.py --check  re-render to a temp dir and compare sha256s with the committed files (exit 1 on any difference)

Rendered with matplotlib mathtext (no TeX needed), 512x160 px, white, DejaVu Sans 28 pt.
"""
import hashlib
import json
import re
import sys
import tempfile
from pathlib import Path

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt  # noqa: E402

HERE = Path(__file__).resolve().parent
FS = 28

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


def new_fig():
    return plt.figure(figsize=(5.12, 1.6), dpi=100, facecolor="white")


def save(fig, path):
    fig.savefig(path, dpi=100, facecolor="white", metadata={"Software": None})
    plt.close(fig)


def mathtext(line):
    # mathtext knows \leq/\geq but not \le/\ge; the canonical LaTeX in items.json keeps the short forms.
    return re.sub(r"\\(le|ge)(?![a-z])", r"\\\1q", line)


def render_printed(path, lines):
    lines = [mathtext(ln) for ln in lines]
    fig = new_fig()
    if len(lines) == 1:
        fig.text(0.5, 0.5, lines[0], fontsize=FS, ha="center", va="center")
    else:
        for y, ln in zip((0.68, 0.32), lines):
            fig.text(0.5, y, ln, fontsize=FS, ha="center", va="center")
    save(fig, path)


def render_diagram(path):
    fig = new_fig()
    ax = fig.add_axes([0.3, 0.1, 0.4, 0.8])
    A, B, C = (0, 0), (1.6, 0), (0, 1)
    xs, ys = zip(A, B, C, A)
    ax.plot(xs, ys, color="black", linewidth=2)
    ax.plot([0, 0.15, 0.15], [0.15, 0.15, 0], color="black", linewidth=1.5)
    for label, (x, y), dx, dy in (("A", A, -0.18, -0.12), ("B", B, 0.1, -0.12), ("C", C, -0.18, 0.04)):
        ax.text(x + dx, y + dy, label, fontsize=FS - 6, ha="center", va="center")
    ax.set_xlim(-0.4, 1.9)
    ax.set_ylim(-0.3, 1.2)
    ax.axis("off")
    save(fig, path)


def render_prose(path):
    fig = new_fig()
    for y, ln in zip((0.78, 0.5, 0.22), ("Shopping list for Saturday:", "milk, eggs, bread, butter,", "apples and a jar of honey.")):
        fig.text(0.5, y, ln, fontsize=FS - 4, ha="center", va="center")
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
    return items


def sha(p: Path):
    return hashlib.sha256(p.read_bytes()).hexdigest()


def dump(obj):
    return json.dumps(obj, indent=2, ensure_ascii=False) + "\n"


def main():
    if "--check" in sys.argv:
        with tempfile.TemporaryDirectory() as td:
            items = build(Path(td))
            bad = []
            manifest = json.loads((HERE / "manifest.json").read_text())
            for it in items:
                got = sha(Path(td) / it["file"])
                committed = HERE / it["file"]
                if not committed.exists() or sha(committed) != got or manifest["files"].get(it["file"]) != got:
                    bad.append(it["file"])
            if (HERE / "items.json").read_text() != dump({"version": 1, "items": items}):
                bad.append("items.json")
        if bad:
            print("DIFFERENT:", ", ".join(bad))
            sys.exit(1)
        print("ok: 18 PNGs and items.json reproduce byte-for-byte")
        return
    items = build(HERE)
    (HERE / "items.json").write_text(dump({"version": 1, "items": items}))
    files = {it["file"]: sha(HERE / it["file"]) for it in items}
    (HERE / "manifest.json").write_text(dump({"version": 1, "count": len(files), "files": files}))
    print(f"wrote {len(files)} PNGs, items.json, manifest.json")


if __name__ == "__main__":
    main()
