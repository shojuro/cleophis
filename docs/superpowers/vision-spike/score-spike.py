#!/usr/bin/env python3
"""score-spike.py — normalised exact-match scoring for the vision spike's JSONL records.

Usage: score-spike.py <items.json> <records.jsonl> [...]
Prints, per file: n records, exact count per rep, p50/p95 total_ms, encode/prefill/decode
medians, max VmHWM, and one line per record with the normalised output on a miss.

Normalisation (pinned here; Stage 3 moves it to probes/vision/normalise.mjs):
  1. drop DocTags <loc_N> tokens
  2. drop \\begin{array}{..} / \\end{array} wrappers, \\left \\right \\, \; \\! \\displaystyle, \\dfrac->\\frac
  3. drop label text: \\text{...}, \\colon, a leading run of letters/spaces ending in ':' ("Solve for x :")
  4. unwrap grouping braces that are not an argument of ^ _ \\frac \\sqrt (parser), keep argument braces,
     write single-character ^/_ arguments without braces (x^2)
  5. drop all whitespace
Exact = normalised output == normalised ground truth.
"""
import json, re, statistics, sys

CMDS_ONE = {"sqrt", "text", "mathrm", "overline", "hat", "vec", "bar"}
CMDS_TWO = {"frac", "binom"}

def tokenize(s):
    i, out = 0, []
    while i < len(s):
        c = s[i]
        if c == "\\":
            m = re.match(r"\\([A-Za-z]+|\\|.)", s[i:])
            out.append(m.group(0)); i += len(m.group(0))
        elif c == "{":
            depth, j = 1, i + 1
            while j < len(s) and depth:
                depth += {"{": 1, "}": -1}.get(s[j], 0); j += 1
            out.append(("g", tokenize(s[i + 1:j - 1]))); i = j
        elif c == "}":
            i += 1  # stray closer (a truncated output): ignore
        else:
            out.append(c); i += 1
    return out

def strip_ws(tokens):
    out = []
    for t in tokens:
        if isinstance(t, tuple): out.append(("g", strip_ws(t[1])))
        elif not t.isspace(): out.append(t)
    return out

def render(tokens):
    out, want = [], 0   # want = how many argument groups the previous token still expects
    for t in strip_ws(tokens):
        if isinstance(t, tuple):
            inner = render(t[1])
            if want:
                prev = out[-1] if out else ""
                if prev in ("^", "_") and len(inner) == 1:
                    out.append(inner)            # x^{2} -> x^2
                else:
                    out.append("{" + inner + "}")
                want -= 1
            else:
                out.append(inner)                # grouping braces: unwrap
            continue
        if t in ("^", "_"):
            want = 1
        elif t.startswith("\\") and t[1:] in CMDS_TWO:
            want = 2
        elif t.startswith("\\") and t[1:] in CMDS_ONE:
            want = 1
        else:
            want = 0 if not (want and len(t) == 1 and out and out[-1] in ("^", "_")) else want - 1
        out.append(t)
    return "".join(out)

def normalise(s):
    s = re.sub(r"<loc_\d+>", " ", s)
    s = re.sub(r"\\begin\{array\}\s*\{[^}]*\}", " ", s)
    s = re.sub(r"\\end(\s*\{[^}]*\}?)?", " ", s)      # tolerant: a token cap may cut "\end{array}" short
    s = re.sub(r"\\(left|right|displaystyle)\b", " ", s)
    s = re.sub(r"\\[,;!]", " ", s)
    s = s.replace("\\dfrac", "\\frac").replace("\\tfrac", "\\frac")
    s = re.sub(r"\\text\s*\{[^}]*\}", " ", s)
    s = re.sub(r"\\colon", ":", s)
    # a label such as "Solve for x :" or "S o l v e :" before the maths
    s = re.sub(r"^[\sA-Za-z]*:\s*", "", s.strip())
    s = re.sub(r"(?m)^[\sA-Za-z]*:\s*$", "", s)       # a label line on its own
    s = s.replace("\n", " ")
    s = render(tokenize(s))
    return re.sub(r"\s+", "", s)

def pct(xs, p):
    xs = sorted(xs); return xs[min(len(xs) - 1, int(round(p * (len(xs) - 1))))]

def main():
    items = {it["id"]: it for it in json.load(open(sys.argv[1]))["items"]}
    for path in sys.argv[2:]:
        rows = [json.loads(l) for l in open(path) if l.strip()]
        ok = [r for r in rows if "error" not in r and r.get("raw") is not None]
        print(f"## {path.split('/')[-1]}: {len(rows)} records, {len(ok)} ok")
        if not ok: continue
        reps = sorted({r.get("rep", 0) for r in ok})
        exact_by_rep = {}
        for r in ok:
            gt = normalise(items[r["id"]]["ground_truth_latex"]); got = normalise(r["raw"])
            r["_exact"] = gt == got; r["_got"] = got; r["_gt"] = gt
            exact_by_rep.setdefault(r.get("rep", 0), []).append(r["_exact"])
        tot = [r["total_ms"] for r in ok]
        print("   exact per rep: " + ", ".join(f"rep{k} {sum(v)}/{len(v)}" for k, v in sorted(exact_by_rep.items()))
              + f" | total_ms p50 {pct(tot,0.5)} p95 {pct(tot,0.95)}"
              + f" | median encode {statistics.median([r.get('encode_ms') or 0 for r in ok]):.0f}"
              + f" prefill {statistics.median([r['prefill_ms'] for r in ok]):.0f}"
              + f" decode {statistics.median([r['decode_ms'] for r in ok]):.0f} ms"
              + f" | img_tok {sorted({r['n_image_tokens'] for r in ok})} | max VmHWM {max(r['vmhwm_kb'] for r in ok)} kB")
        for r in ok:
            flag = "EXACT" if r["_exact"] else "MISS "
            line = f"   {flag} {r['id']:<14} rep{r.get('rep',0)} tot {r['total_ms']:>6} ms"
            if not r["_exact"]: line += f"  got {r['_got']!r}  want {r['_gt']!r}"
            print(line)

if __name__ == "__main__":
    main()
